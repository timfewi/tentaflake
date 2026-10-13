#!/usr/bin/env python3
"""Validate the v1 contract and fixtures without starting a host or fetching refs."""

import json
import re
import sys
from pathlib import Path

try:
    from jsonschema import Draft202012Validator, FormatChecker
    from referencing import Registry
    from referencing.exceptions import NoSuchResource
except ImportError as error:
    raise SystemExit(
        "API validation requires the pinned contributor environment: just api-contract"
    ) from error


ROOT = Path(__file__).resolve().parents[1]
CONTRACT = ROOT / "api/v1/openapi.json"
FIXTURES = ROOT / "api/v1/fixtures.json"
METHODS = {"get", "post", "put", "patch", "delete"}
PUBLIC = {
    "getMeta",
    "createSession",
    "getOidcChallenge",
    "beginOidc",
    "finishOidc",
}
EPHEMERAL = {"createSession", "deleteSession", "createPlan", "previewChange", "previewImport"}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def reject_remote(uri):
    raise NoSuchResource(ref=uri)


def load(path):
    def no_duplicates(pairs):
        value = {}
        for key, item in pairs:
            require(key not in value, f"{path.name}: duplicate JSON key {key}")
            value[key] = item
        return value

    return json.loads(path.read_text(), object_pairs_hook=no_duplicates)


def resolve(document, reference):
    require(reference.startswith("#/"), f"Nonlocal reference forbidden: {reference}")
    value = document
    for part in reference[2:].split("/"):
        value = value[part.replace("~1", "/").replace("~0", "~")]
    return value


def walk(document, value):
    if isinstance(value, dict):
        if "$ref" in value:
            resolve(document, value["$ref"])
        for item in value.values():
            yield from walk(document, item)
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from walk(document, item)


def validator(document, schema):
    # Retain the document root for its local OpenAPI component references.
    root = {"components": document["components"], **schema}
    return Draft202012Validator(
        root, format_checker=FormatChecker(), registry=Registry(retrieve=reject_remote)
    )


def validate(document, schema, value, name):
    errors = sorted(validator(document, schema).iter_errors(value), key=lambda e: str(e.path))
    require(not errors, f"{name}: {errors[0].message if errors else ''}")


def dereference(document, value):
    return resolve(document, value["$ref"]) if "$ref" in value else value


def check_contract(document):
    require(document["openapi"] == "3.1.0", "Expected OpenAPI 3.1")
    require(
        document["x-implementation"] == {
            "status": "unavailable", "runtimeAcceptance": False, "issue": 152
        },
        "Fixtures must not be promoted to backend acceptance",
    )
    require(document["servers"] == [{"url": "/", "description": "Private same-origin HTTPS endpoint"}],
            "Host API must retain its private same-origin transport")
    require(document["security"] == [{"sessionCookie": []}, {"cliBearer": []}],
            "Session and CLI authority must be explicit")
    for node in walk(document, document):
        require("$id" not in node, "Nested schema identifiers may not alter reference scope")
        require("$dynamicRef" not in node, "Dynamic schema references are not supported by this local contract")
        if "schema" in node:
            Draft202012Validator.check_schema(node["schema"])
    for name, schema in document["components"]["schemas"].items():
        Draft202012Validator.check_schema(schema)
        if schema.get("type") == "object" and "properties" in schema:
            require(schema.get("additionalProperties") is False,
                    f"{name}: wire objects must reject undeclared authority fields")

    operations = {}
    for path, item in document["paths"].items():
        require(path == "/api/meta" or path.startswith("/api/v1/"), f"Unversioned route: {path}")
        for method, operation in item.items():
            require(method in METHODS, f"Unexpected path item {method}")
            name = operation["operationId"]
            require(name not in operations, f"Duplicate operationId: {name}")
            operations[name] = (path, method, operation)
            require(operation.get("security") == [] if name in PUBLIC else "security" not in operation,
                    f"{name}: unexpected authentication override")
            if "x-required-action" in operation:
                validate(document, {"$ref": "#/components/schemas/Action"},
                         operation["x-required-action"], name + " grant action")
            parameters = [dereference(document, p) for p in operation.get("parameters", [])]
            keys = [(p["in"], p["name"]) for p in parameters]
            require(len(keys) == len(set(keys)), f"{name}: duplicate parameter")
            expected = set(re.findall(r"{(\w+)}", path))
            actual = {p["name"] for p in parameters if p["in"] == "path" and p.get("required")}
            require(actual == expected, f"{name}: missing/extraneous path parameter")
            if method != "get" and name not in EPHEMERAL:
                require(operation.get("x-durable-mutation") is True,
                        f"{name}: mutation lacks durable receipt")
            if operation.get("x-durable-mutation"):
                for header in ("Idempotency-Key", "Idempotency-Epoch"):
                    require(any(p["name"] == header and p.get("required") for p in parameters),
                            f"{name}: missing required {header}")
                for status, response in operation["responses"].items():
                    if status.startswith("2"):
                        require("Operation-Id" in response["headers"], f"{name}: missing receipt header")
            if method != "get" and name not in PUBLIC:
                require(("header", "X-Tentaflake-CSRF") in keys, f"{name}: no CSRF contract")
            require("default" in operation["responses"], f"{name}: no failure envelope")
    return operations


def check_exchange(document, operation, case):
    name = case["name"]
    request = case["request"]
    groups = {"path": request["pathParameters"], "query": request["query"], "header": request["headers"]}
    parameters = [dereference(document, p) for p in operation.get("parameters", [])]
    for parameter in parameters:
        group = groups[parameter["in"]]
        key = parameter["name"]
        require(not parameter.get("required") or key in group, f"{name}: required parameter {key}")
        if key in group:
            validate(document, parameter["schema"], group[key], f"{name}: {key}")
    for location, values in groups.items():
        allowed = {p["name"] for p in parameters if p["in"] == location}
        require(set(values) <= allowed, f"{name}: undeclared {location} parameter")
    if name in ("writeFile", "uploadFile"):
        h = request["headers"]
        require(("If-Match" in h) != ("If-None-Match" in h), f"{name}: exactly one write precondition")
    if "requestBody" in operation:
        media = request["contentType"]
        validate(document, operation["requestBody"]["content"][media]["schema"], request["body"], name + " request")
    else:
        require("body" not in request, f"{name}: unexpected request body")
    response = case["response"]
    spec = operation["responses"][str(response["status"])]
    for header, value in response["headers"].items():
        validate(document, dereference(document, spec["headers"][header])["schema"], value, name + " response header")
    require(response["headers"].get("Cache-Control") == "no-store", f"{name}: sensitive response cache")
    if operation.get("x-durable-mutation"):
        require("Operation-Id" in response["headers"], f"{name}: no fixture receipt")
    if name in ("readFile", "downloadFile"):
        require(response["headers"].get("Content-Disposition") == "attachment",
                f"{name}: workspace bytes must be downloaded")
        require(response["headers"].get("X-Content-Type-Options") == "nosniff",
                f"{name}: workspace bytes must not be sniffed")
        require(response["headers"].get("Content-Length") == len(response["body"].encode("utf-8")),
                f"{name}: immutable snapshot size mismatch")
    if "content" in spec:
        validate(document, spec["content"][response["contentType"]]["schema"], response["body"], name + " response")
        if response["contentType"] == "application/json" and "ETag" in spec["headers"]:
            require(response["headers"].get("ETag") == '"' + response["body"]["revision"] + '"',
                    f"{name}: strong response ETag and body revision differ")
    else:
        require("body" not in response, f"{name}: unexpected response body")


def main():
    require("date-time" in FormatChecker.checkers,
            "Missing date-time format validation; use the pinned jsonschema/rfc3339-validator environment")
    require("uuid" in FormatChecker.checkers, "Missing UUID format validation")
    document, fixtures = load(CONTRACT), load(FIXTURES)
    operations = check_contract(document)
    require(fixtures["contract"] == CONTRACT.name and fixtures["backend"] == "unavailable",
            "Fixture provenance must remain explicit")
    seen = set()
    for case in fixtures["exchanges"]:
        name = case["name"]
        require(name not in seen, f"Duplicate exchange fixture: {name}")
        seen.add(name)
        path, method, operation = operations[name]
        require(case["request"]["path"] == path and case["request"]["method"] == method.upper(),
                f"{name}: fixture route drift")
        check_exchange(document, operation, case)
    require(seen == set(operations), f"Missing route fixtures: {set(operations) - seen}")
    for case in fixtures["valid"]:
        validate(document, {"$ref": "#/components/schemas/" + case["schema"]}, case["value"], case["name"])
    for case in fixtures["invalid"]:
        schema = {"$ref": "#/components/schemas/" + case["schema"]}
        require(not validator(document, schema).is_valid(case["value"]),
                f"Counterexample wrongly accepted: {case['name']}")
    websocket = operations["attachTerminal"][2]["x-websocket"]
    for direction, schema in (("clientFrame", "TerminalInput"), ("serverFrame", "TerminalOutput")):
        require(websocket[direction] == {"$ref": "#/components/schemas/" + schema},
                "WebSocket frames must use the shared contract schemas")
    print(f"API contract: {len(operations)} routes, {len(fixtures['valid'])} valid examples, "
          f"{len(fixtures['invalid'])} rejected counterexamples; all local refs and schemas valid. "
          "No backend/runtime acceptance.")


if __name__ == "__main__":
    try:
        main()
    except (AssertionError, KeyError, ValueError) as error:
        print(f"API contract validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
