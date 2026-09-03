#!/usr/bin/env python3
"""Verify the Sui agent-attestation V1 signing fixture without dependencies."""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import sys
from pathlib import Path
from typing import Any

# Ed25519 parameters from RFC 8032.
_FIELD_PRIME = 2**255 - 19
_SUBGROUP_ORDER = 2**252 + 27742317777372353535851937790883648493
_CURVE_D = (-121665 * pow(121666, _FIELD_PRIME - 2, _FIELD_PRIME)) % _FIELD_PRIME
_SQRT_MINUS_ONE = pow(2, (_FIELD_PRIME - 1) // 4, _FIELD_PRIME)
_IDENTITY = (0, 1, 1, 0)


class VectorError(ValueError):
    """The fixture or a derived cryptographic value is invalid."""


def _inverse(value: int) -> int:
    return pow(value, _FIELD_PRIME - 2, _FIELD_PRIME)


def _recover_x(y: int, sign: int) -> int:
    if y >= _FIELD_PRIME:
        raise VectorError("non-canonical Ed25519 y-coordinate")
    y_squared = y * y % _FIELD_PRIME
    x_squared = (
        (y_squared - 1)
        * _inverse((_CURVE_D * y_squared + 1) % _FIELD_PRIME)
        % _FIELD_PRIME
    )
    x = pow(x_squared, (_FIELD_PRIME + 3) // 8, _FIELD_PRIME)
    if (x * x - x_squared) % _FIELD_PRIME != 0:
        x = x * _SQRT_MINUS_ONE % _FIELD_PRIME
    if (x * x - x_squared) % _FIELD_PRIME != 0:
        raise VectorError("compressed Ed25519 point is not on the curve")
    if x == 0 and sign:
        raise VectorError("non-canonical Ed25519 sign bit")
    if (x & 1) != sign:
        x = _FIELD_PRIME - x
    return x


def _decode_point(encoded: bytes) -> tuple[int, int, int, int]:
    if len(encoded) != 32:
        raise VectorError("compressed Ed25519 point must be 32 bytes")
    encoded_y = int.from_bytes(encoded, "little")
    sign = encoded_y >> 255
    y = encoded_y & ((1 << 255) - 1)
    x = _recover_x(y, sign)
    return (x, y, 1, x * y % _FIELD_PRIME)


def _point_add(
    left: tuple[int, int, int, int],
    right: tuple[int, int, int, int],
) -> tuple[int, int, int, int]:
    x1, y1, z1, t1 = left
    x2, y2, z2, t2 = right
    a = (y1 - x1) * (y2 - x2) % _FIELD_PRIME
    b = (y1 + x1) * (y2 + x2) % _FIELD_PRIME
    c = 2 * _CURVE_D * t1 * t2 % _FIELD_PRIME
    d = 2 * z1 * z2 % _FIELD_PRIME
    e = b - a
    f = d - c
    g = d + c
    h = b + a
    return (
        e * f % _FIELD_PRIME,
        g * h % _FIELD_PRIME,
        f * g % _FIELD_PRIME,
        e * h % _FIELD_PRIME,
    )


def _scalar_multiply(
    point: tuple[int, int, int, int],
    scalar: int,
) -> tuple[int, int, int, int]:
    result = _IDENTITY
    addend = point
    while scalar:
        if scalar & 1:
            result = _point_add(result, addend)
        addend = _point_add(addend, addend)
        scalar >>= 1
    return result


def _points_equal(
    left: tuple[int, int, int, int],
    right: tuple[int, int, int, int],
) -> bool:
    x1, y1, z1, _ = left
    x2, y2, z2, _ = right
    return (
        (x1 * z2 - x2 * z1) % _FIELD_PRIME == 0
        and (y1 * z2 - y2 * z1) % _FIELD_PRIME == 0
    )


def _is_identity(point: tuple[int, int, int, int]) -> bool:
    return _points_equal(point, _IDENTITY)


_BASE_Y = 4 * _inverse(5) % _FIELD_PRIME
_BASE_X = _recover_x(_BASE_Y, 0)
_BASE_POINT = (_BASE_X, _BASE_Y, 1, _BASE_X * _BASE_Y % _FIELD_PRIME)


def verify_raw_ed25519(public_key: bytes, message: bytes, signature: bytes) -> bool:
    """Strictly verify a raw RFC 8032 Ed25519 signature."""

    if len(public_key) != 32 or len(signature) != 64:
        return False

    encoded_r = signature[:32]
    scalar_s = int.from_bytes(signature[32:], "little")
    if scalar_s >= _SUBGROUP_ORDER:
        return False

    try:
        public_point = _decode_point(public_key)
        r_point = _decode_point(encoded_r)
    except VectorError:
        return False

    # Reject identity and non-prime-subgroup points rather than accepting
    # cofactor-related edge cases that different Ed25519 libraries treat
    # inconsistently.
    if (
        _is_identity(public_point)
        or _is_identity(r_point)
        or not _is_identity(_scalar_multiply(public_point, _SUBGROUP_ORDER))
        or not _is_identity(_scalar_multiply(r_point, _SUBGROUP_ORDER))
    ):
        return False

    challenge = int.from_bytes(
        hashlib.sha512(encoded_r + public_key + message).digest(), "little"
    ) % _SUBGROUP_ORDER
    left = _scalar_multiply(_BASE_POINT, scalar_s)
    right = _point_add(r_point, _scalar_multiply(public_point, challenge))
    return _points_equal(left, right)


def _require_mapping(value: Any, name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise VectorError(f"{name} must be a JSON object")
    return value


def _require_text(mapping: dict[str, Any], name: str) -> str:
    value = mapping.get(name)
    if not isinstance(value, str):
        raise VectorError(f"{name} must be a string")
    return value


def _require_u64(mapping: dict[str, Any], name: str) -> int:
    value = mapping.get(name)
    if isinstance(value, bool) or not isinstance(value, int):
        raise VectorError(f"{name} must be an integer")
    if not 0 <= value < 2**64:
        raise VectorError(f"{name} is outside the u64 range")
    return value


def _decode_hex(mapping: dict[str, Any], name: str, length: int) -> bytes:
    encoded = _require_text(mapping, name)
    try:
        value = bytes.fromhex(encoded)
    except ValueError as error:
        raise VectorError(f"{name} is not valid hexadecimal") from error
    if len(value) != length:
        raise VectorError(f"{name} must decode to {length} bytes")
    return value


def _decode_id(mapping: dict[str, Any], name: str) -> bytes:
    encoded = _require_text(mapping, name)
    if not encoded.startswith("0x"):
        raise VectorError(f"{name} must start with 0x")
    try:
        value = bytes.fromhex(encoded[2:])
    except ValueError as error:
        raise VectorError(f"{name} is not valid hexadecimal") from error
    if len(value) != 32:
        raise VectorError(f"{name} must encode one 32-byte Sui ID")
    return value


def _uleb128(value: int) -> bytes:
    if value < 0:
        raise VectorError("ULEB128 cannot encode a negative value")
    encoded = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            byte |= 0x80
        encoded.append(byte)
        if not value:
            return bytes(encoded)


def _bcs_vector(value: bytes) -> bytes:
    return _uleb128(len(value)) + value


def _bcs_u64(value: int) -> bytes:
    return value.to_bytes(8, "little")


def reconstruct_payload(fields: dict[str, Any]) -> bytes:
    """Reconstruct the exact field order used by Move SigningPayload."""

    domain = _require_text(fields, "domain_utf8").encode("utf-8")
    network_domain = _require_text(fields, "network_domain_utf8").encode("utf-8")
    parts = [
        _bcs_vector(domain),
        _bcs_vector(network_domain),
        _decode_id(fields, "registry_id_hex"),
        _decode_id(fields, "agent_record_id_hex"),
        _bcs_u64(_require_u64(fields, "protocol_version")),
        _bcs_u64(_require_u64(fields, "issuer_key_id")),
        _bcs_u64(_require_u64(fields, "issuer_set_epoch")),
        _bcs_u64(_require_u64(fields, "sequence")),
        _bcs_u64(_require_u64(fields, "not_before_epoch")),
        _bcs_u64(_require_u64(fields, "expires_epoch")),
        _bcs_vector(_decode_hex(fields, "policy_digest_hex", 32)),
        _bcs_vector(_decode_hex(fields, "image_digest_hex", 32)),
        _bcs_vector(_decode_hex(fields, "golden_eval_report_digest_hex", 32)),
        _bcs_vector(_decode_hex(fields, "evidence_digest_hex", 32)),
    ]
    return b"".join(parts)


def verify_fixture(path: Path) -> None:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise VectorError(f"cannot read fixture {path}: {error}") from error

    root = _require_mapping(document, "fixture")
    if root.get("schema") != "tentaflake.sui-agent-attestation.signing-vector.v1":
        raise VectorError("unexpected fixture schema")

    key_material = _require_mapping(root.get("key_material"), "key_material")
    if key_material.get("private_seed_included") is not False:
        raise VectorError("fixture must explicitly exclude private seed material")

    fields = _require_mapping(root.get("fields"), "fields")
    expected = _require_mapping(root.get("expected"), "expected")
    payload = reconstruct_payload(fields)

    expected_length = _require_u64(expected, "payload_length")
    expected_payload = _decode_hex(expected, "payload_hex", expected_length)
    if len(payload) != expected_length:
        raise VectorError(
            f"payload length mismatch: reconstructed {len(payload)}, "
            f"expected {expected_length}"
        )
    if not hmac.compare_digest(payload, expected_payload):
        raise VectorError(
            "reconstructed BCS payload mismatch:\n"
            f"  actual:   {payload.hex()}\n"
            f"  expected: {expected_payload.hex()}"
        )
    print(f"PASS BCS payload: {len(payload)} bytes match expected payload_hex")

    public_key = _decode_hex(expected, "public_key_hex", 32)
    signature = _decode_hex(expected, "signature_hex", 64)
    if not verify_raw_ed25519(public_key, payload, signature):
        raise VectorError("raw Ed25519 signature did not verify")
    print("PASS raw Ed25519 signature: expected public key verifies payload")

    mutated = bytearray(payload)
    mutated[-1] ^= 0x01
    if verify_raw_ed25519(public_key, bytes(mutated), signature):
        raise VectorError("signature unexpectedly verified after one-byte mutation")
    print(
        "PASS mutation rejection: XOR of final payload byte with 0x01 "
        "invalidates signature"
    )


def main() -> int:
    repository_root = Path(__file__).resolve().parents[1]
    default_fixture = (
        repository_root
        / "integrations"
        / "sui-agent-attestation"
        / "test-vectors"
        / "signing-payload-v1.json"
    )
    parser = argparse.ArgumentParser(
        description="Verify the Sui attestation V1 BCS and Ed25519 fixture."
    )
    parser.add_argument(
        "fixture",
        nargs="?",
        type=Path,
        default=default_fixture,
        help=f"fixture path (default: {default_fixture})",
    )
    arguments = parser.parse_args()

    try:
        verify_fixture(arguments.fixture)
    except VectorError as error:
        print(f"FAIL {error}", file=sys.stderr)
        return 1
    print("PASS Sui attestation signing vector V1")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
