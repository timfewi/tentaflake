import assert from "node:assert/strict";
import childProcess from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import { fileURLToPath } from "node:url";
import { once } from "node:events";
import {
  chmodSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import http from "node:http";
import net from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { createModels } from "@earendil-works/pi-ai/models";
import { openaiProvider } from "@earendil-works/pi-ai/providers/openai";
import { anthropicProvider } from "@earendil-works/pi-ai/providers/anthropic";
import { opencodeGoProvider } from "@earendil-works/pi-ai/providers/opencode-go";
import { Engine } from "../engine.mjs";
import { Ledger } from "../ledger.mjs";
import { projection } from "../runner.mjs";

const sse = (event) =>
  `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`;
const toolArguments = JSON.stringify({ arguments: '{"q":"local"}' });
function responsesTool(model) {
  const item = {
    type: "function_call",
    id: "fc_fixture",
    call_id: "call_fixture",
    name: "research_search",
    arguments: toolArguments,
    status: "completed",
  };
  const response = {
    object: "response",
    id: "resp_tool",
    model,
    status: "completed",
    output: [item],
    usage: { input_tokens: 8, output_tokens: 2, total_tokens: 10 },
  };
  return [
    {
      type: "response.created",
      response: { ...response, status: "in_progress", output: [], usage: null },
    },
    {
      type: "response.output_item.added",
      output_index: 0,
      item: { ...item, arguments: "", status: "in_progress" },
    },
    {
      type: "response.function_call_arguments.delta",
      output_index: 0,
      item_id: item.id,
      delta: toolArguments,
    },
    {
      type: "response.function_call_arguments.done",
      output_index: 0,
      item_id: item.id,
      arguments: toolArguments,
    },
    { type: "response.output_item.done", output_index: 0, item },
    { type: "response.completed", response },
  ]
    .map((event, sequence_number) => sse({ ...event, sequence_number }))
    .join("");
}
function responses(model) {
  const item = {
    type: "message",
    role: "assistant",
    id: "msg_1",
    status: "completed",
    content: [
      { type: "output_text", text: "ready", annotations: [], logprobs: [] },
    ],
  };
  const response = {
    object: "response",
    id: "resp_1",
    model,
    status: "completed",
    output: [item],
    usage: { input_tokens: 8, output_tokens: 2, total_tokens: 10 },
  };
  return [
    {
      type: "response.created",
      response: { ...response, status: "in_progress", output: [], usage: null },
    },
    {
      type: "response.output_item.added",
      output_index: 0,
      item: { ...item, content: [], status: "in_progress" },
    },
    {
      type: "response.content_part.added",
      output_index: 0,
      content_index: 0,
      item_id: item.id,
      part: { ...item.content[0], text: "" },
    },
    {
      type: "response.output_text.delta",
      output_index: 0,
      content_index: 0,
      item_id: item.id,
      delta: "ready",
    },
    { type: "response.output_item.done", output_index: 0, item },
    { type: "response.completed", response },
  ]
    .map((event, sequence_number) => sse({ ...event, sequence_number }))
    .join("");
}
function messages(model, tool) {
  const block = tool
    ? {
        type: "tool_use",
        id: "toolu_fixture",
        name: "research_search",
        input: {},
      }
    : { type: "text", text: "" };
  const value = tool
    ? {
        type: "input_json_delta",
        partial_json: JSON.stringify({ arguments: '{"q":"local"}' }),
      }
    : { type: "text_delta", text: "ready" };
  return [
    {
      type: "message_start",
      message: {
        id: tool ? "msg_1" : "msg_2",
        model,
        type: "message",
        role: "assistant",
        content: [],
        usage: { input_tokens: tool ? 8 : 12, output_tokens: 0 },
      },
    },
    { type: "content_block_start", index: 0, content_block: block },
    { type: "content_block_delta", index: 0, delta: value },
    { type: "content_block_stop", index: 0 },
    {
      type: "message_delta",
      delta: {
        stop_reason: tool ? "tool_use" : "end_turn",
        stop_sequence: null,
      },
      usage: { output_tokens: tool ? 2 : 3 },
    },
    { type: "message_stop" },
  ]
    .map(sse)
    .join("");
}
function completions(model, tool) {
  const chunk = {
    id: "chatcmpl_1",
    model,
    object: "chat.completion.chunk",
    created: 10,
  };
  const delta = tool
    ? {
        role: "assistant",
        tool_calls: [
          {
            index: 0,
            id: "call_fixture",
            type: "function",
            function: { name: "research_search", arguments: toolArguments },
          },
        ],
      }
    : { role: "assistant", content: "ready" };
  return (
    [
      {
        ...chunk,
        choices: [{ index: 0, delta, finish_reason: null }],
      },
      {
        ...chunk,
        choices: [
          { index: 0, delta: {}, finish_reason: tool ? "tool_calls" : "stop" },
        ],
      },
      {
        ...chunk,
        choices: [],
        usage: { prompt_tokens: 8, completion_tokens: 2, total_tokens: 10 },
      },
    ]
      .map((x) => "data: " + JSON.stringify(x) + "\n\n")
      .join("") + "data: [DONE]\n\n"
  );
}

const definitions = [
  [
    "openai",
    "openai-responses",
    "openai-responses",
    "/v1/responses",
    ["research_search", "research_read"],
  ],
  [
    "anthropic",
    "anthropic-messages",
    "anthropic-messages",
    "/v1/messages",
    ["research_search"],
  ],
  [
    "opencode-go",
    "openai-completions",
    "opencode-go-chat-completions",
    "/v1/chat/completions",
    ["research_search"],
  ],
];
test("installed SDK binds individual grants, mediates each generation and projects actual terminal measurements", async (t) => {
  const dir = mkdtempSync(join(tmpdir(), "pi-provider-grants-"));
  chmodSync(dir, 0o700);
  const virtual = join(dir, "virtual"),
    researchCommand =
      "/nix/store/synthetic-provider-grant-test/bin/research-client";
  const spawn = childProcess.spawn;
  const binary = process.env.TENTAFLAKE_TEST_BROKER;
  if (!binary) {
    rmSync(dir, { recursive: true });
    return t.skip("requires the packaged Rust broker");
  }
  const oldMask = process.umask(0o077);
  t.after(() => process.umask(oldMask));
  const catalog = createModels();
  for (const provider of [
    openaiProvider(),
    anthropicProvider(),
    opencodeGoProvider(),
  ])
    catalog.setProvider(provider);
  const selections = definitions.map(
    ([provider, api, protocol, path, tools], i) => ({
      provider,
      protocol,
      path,
      tools,
      revision: "fixture-" + i,
      model: catalog.getModels(provider).find((m) => m.api === api).id,
    }),
  );
  const origins = [],
    servers = [],
    seen = [],
    wire = [];
  const fetchNative = globalThis.fetch;
  let proc, ledger, engine;
  let brokerError = "";
  let failingProvider;
  try {
    for (const selection of selections) {
      const server = http.createServer(async (req, res) => {
        try {
          const chunks = [];
          for await (const chunk of req) chunks.push(chunk);
          const body = JSON.parse(Buffer.concat(chunks));
          const anth = selection.provider === "anthropic";
          assert.equal(
            req.url,
            selection.provider === "opencode-go"
              ? "/openai/v1/chat/completions"
              : selection.path,
          );
          assert.equal(body.model, selection.model);
          assert.equal(body.stream, true);
          assert.equal(body.fallbacks, undefined);
          assert.equal(JSON.stringify(body).includes('"cache_control"'), false);
          assert.equal(
            req.headers[anth ? "x-api-key" : "authorization"],
            (anth ? "" : "Bearer ") + "origin-" + selection.revision,
          );
          assert.equal(req.headers["x-tentaflake-grant"], undefined);
          assert.deepEqual(
            (body.tools ?? []).map((tool) => tool.name ?? tool.function.name),
            selection.tools,
          );
          const first = !seen.includes(selection.revision);
          if (failingProvider === selection.provider) {
            seen.push(selection.revision);
            res.writeHead(503, { "content-type": "application/json" });
            res.end('{"error":{"message":"synthetic provider unavailable"}}');
            return;
          }
          if (selection.provider === "openai" && !first)
            assert.ok(
              body.input.some(
                (v) =>
                  v.type === "function_call_output" &&
                  v.call_id === "call_fixture" &&
                  JSON.stringify(v.output).includes(
                    "fixed fixture observation",
                  ),
              ),
            );
          if (selection.provider === "opencode-go" && !first)
            assert.ok(
              body.messages.some(
                (v) =>
                  v.role === "tool" &&
                  v.tool_call_id === "call_fixture" &&
                  JSON.stringify(v.content).includes(
                    "fixed fixture observation",
                  ),
              ),
            );
          if (anth && !first)
            assert.ok(
              body.messages.some((m) =>
                m.content?.some?.(
                  (v) =>
                    v.type === "tool_result" &&
                    v.tool_use_id === "toolu_fixture",
                ),
              ),
            );
          seen.push(selection.revision);
          const frames = anth
            ? messages(body.model, first)
            : selection.provider === "openai"
              ? first
                ? responsesTool(body.model)
                : responses(body.model)
              : completions(body.model, first);
          res.writeHead(200, { "content-type": "text/event-stream" });
          res.end(frames);
        } catch (error) {
          brokerError += error.stack;
          res.writeHead(500);
          res.end();
        }
      });
      server.listen(0, "127.0.0.1");
      await once(server, "listening");
      servers.push(server);
      origins.push(
        "http://127.0.0.1:" +
          server.address().port +
          (selection.provider === "opencode-go" ? "/openai" : "") +
          selection.path,
      );
    }
    const reservation = net.createServer();
    reservation.listen(0, "127.0.0.1");
    await once(reservation, "listening");
    const port = reservation.address().port;
    await new Promise((resolve) => reservation.close(resolve));
    const brokerBase = "http://10.88.0.1:" + port + "/v1",
      grants = selections.map(({ path, ...sel }) => ({
        ...sel,
        version: 1,
        brokerBinding: true,
        endpoint: brokerBase.slice(0, -3) + path,
        budget: { max_output_tokens: 1024 },
        pricing: {
          revision: "fixture-price",
          currency: "USD",
          input_microusd_per_million: 1,
          output_microusd_per_million: 1,
        },
      }));
    const config = {
      agent: "fixture",
      listen: "127.0.0.1:" + port,
      token_file: virtual,
      audit_file: join(dir, "audit"),
      budget_state_file: join(dir, "budget"),
      llm: {
        allow_plain_http_for_tests: true,
        max_completion_tokens: 1024,
        streaming: { enable: true },
        grants: grants.map((g, i) => {
          const credential = join(dir, g.revision);
          writeFileSync(credential, "origin-" + g.revision, { mode: 0o600 });
          return {
            version: 1,
            revision: g.revision,
            provider: g.provider,
            protocol: g.protocol,
            upstream_endpoint: origins[i],
            provider_credential_file: credential,
            model: {
              name: g.model,
              input_microusd_per_million: 1,
              output_microusd_per_million: 1,
            },
            pricing_revision: "fixture-price",
            currency: "USD",
            tools: g.tools,
            max_output_tokens: 1024,
          };
        }),
      },
    };
    writeFileSync(virtual, "capsule-only", { mode: 0o600 });
    writeFileSync(join(dir, "config"), JSON.stringify(config));
    proc = spawn(binary, ["--config", join(dir, "config")], {
      stdio: ["ignore", "ignore", "pipe"],
      env: {},
    });
    proc.stderr.on("data", (chunk) => {
      brokerError = (brokerError + chunk).slice(-2000);
    });
    const ready = Date.now() + 5000;
    for (;;) {
      assert.equal(proc.exitCode, null, brokerError);
      assert.ok(Date.now() < ready, "broker startup deadline " + brokerError);
      const socket = net.connect({ host: "127.0.0.1", port });
      try {
        await once(socket, "connect");
        socket.destroy();
        break;
      } catch {
        socket.destroy();
        await delay(20);
      }
    }
    globalThis.fetch = async (input, init) => {
      const request = new Request(input, init),
        url = new URL(request.url);
      assert.equal(url.host, "10.88.0.1:" + port);
      assert.equal(request.headers.get("authorization"), "Bearer capsule-only");
      const entry = {
        grant: request.headers.get("x-tentaflake-grant"),
        operation: request.headers.get("x-tentaflake-operation"),
        affinity: request.headers.get("x-opencode-session"),
        path: url.pathname,
      };
      wire.push(entry);
      assert.match(entry.operation, /^[0-9a-f]{64}$/);
      const response = await fetchNative(
        "http://127.0.0.1:" + port + url.pathname,
        {
          method: "POST",
          headers: request.headers,
          body: await request.arrayBuffer(),
          signal: request.signal,
        },
      );
      if (!response.ok) brokerError += await response.clone().text();
      return response;
    };
    const state = join(dir, "state");
    mkdirSync(state, { mode: 0o700 });
    ledger = new Ledger(state);
    childProcess.spawn = (command, args, options) => {
      if (command !== researchCommand) return spawn(command, args, options);
      assert.deepEqual(args, ["--socket", "/run/tentaflake-research/socket"]);
      return spawn(
        process.execPath,
        [fileURLToPath(new URL("./provider-mcp-fixture.mjs", import.meta.url))],
        options,
      );
    };
    syncBuiltinESMExports();
    engine = new Engine(
      {
        stateDir: state,
        workspace: dir,
        brokerBaseUrl: brokerBase,
        brokerTokenFile: virtual,
        researchCommand,
        model: grants[0].model,
        grants,
      },
      ledger,
    );
    await engine.open();
    assert.deepEqual(engine.registeredTools(), [
      "research_search",
      "research_read",
    ]);
    for (const [i, grant] of grants.entries()) {
      const sid = "session-" + i;
      await engine.session(sid);
      assert.equal(ledger.hasSession(sid), true);
      assert.equal(ledger.sessionGrant(sid), null);
      const row = ledger.admit({
        request_id: "request-" + i,
        session_id: sid,
        input: "Synthetic local fixture",
        context_mode: "persistent",
        grant: { ...grant, run_budget: { max_tokens: 50000, max_cost_microusd: 100 } },
      }).row;
      await engine.execute(row);
      const publicRow = projection(ledger.get(row.id));
      assert.equal(
        publicRow.state,
        "completed",
        JSON.stringify({ wire, seen, brokerError }),
      );
      assert.equal(publicRow.output, "ready");
      const tools = ledger.events(row.id).filter(event => event.event.startsWith("run.tool_"));
      assert.deepEqual(tools.map(({ event, tool, invocation }) => [event, tool, invocation]), [
        ["run.tool_started", "research_search", 1], ["run.tool_completed", "research_search", 1],
      ]);
      assert.equal(ledger.sessionGrant(sid), grant.revision);
      assert.deepEqual(publicRow.measurement, {
        provider: grant.provider,
        model: grant.model,
        protocol: grant.protocol,
        grant_revision: grant.revision,
        input_tokens: i === 1 ? 20 : 16,
        output_tokens: i === 1 ? 5 : 4,
        provider_requests: 2,
        streamed: true,
        tool_calls: 1,
        tool_results: 1,
      });
      await assert.rejects(
        engine.session(sid, grants[(i + 1) % 3]),
        /binding conflict/,
      );
    }
    const follow = {
      request_id: "go-another-turn",
      session_id: "session-2",
      input: "Continue fixture",
      context_mode: "persistent",
      grant: grants[2],
    };
    const admitted = ledger.admit(follow);
    await engine.execute(admitted.row);
    assert.equal(
      projection(ledger.byRequest(follow.request_id)).state,
      "completed",
    );
    assert.equal(ledger.admit(follow).duplicate, true);
    assert.deepEqual(seen, [
      "fixture-0",
      "fixture-0",
      "fixture-1",
      "fixture-1",
      "fixture-2",
      "fixture-2",
      "fixture-2",
    ]);
    assert.equal(new Set(wire.map((v) => v.operation)).size, 7);
    assert.equal(wire[0].affinity, null);
    assert.match(wire[4].affinity, /^[0-9a-f]{64}$/);
    assert.equal(wire[4].affinity, wire[5].affinity);
    assert.equal(wire[4].affinity, wire[6].affinity);
    const unfunded = ledger.admit({
      ...follow, request_id: "unfunded", grant: { ...grants[2],
        run_budget: { max_tokens: 1, max_cost_microusd: 1 } },
    });
    await engine.execute(unfunded.row);
    const refused = projection(ledger.get(unfunded.row.id));
    assert.equal(refused.state, "failed");
    assert.equal(refused.measurement.provider_requests, 0);
    assert.equal(refused.measurement.input_tokens + refused.measurement.output_tokens, 0);
    assert.equal(refused.measurement.streamed, false);
    assert.equal(wire.length, 7);
    assert.equal(ledger.quarantined, false);
    await engine.close();
    engine = new Engine(engine.config, ledger);
    await engine.open();
    assert.equal(ledger.quarantined, false, "local refusal permits clean recovery");
    await engine.close();
    for (const grant of grants) {
      failingProvider = grant.provider;
      const stateDir = join(dir, "failure-" + grant.revision);
      mkdirSync(stateDir, { mode: 0o700 });
      const isolated = new Ledger(stateDir);
      const worker = new Engine(
        { ...engine.config, stateDir, grants: [grant], model: grant.model },
        isolated,
      );
      try {
        await worker.open();
        const row = isolated.admit({
          request_id: "unavailable",
          session_id: "failure",
          grant,
          input: "Synthetic provider outage",
          context_mode: "persistent",
        }).row;
        const previous = seen.length;
        await worker.execute(row);
        const result = projection(isolated.get(row.id));
        assert.ok(
          ["interrupted", "failed"].includes(result.state),
          JSON.stringify(result),
        );
        assert.equal(Object.hasOwn(result, "measurement"), false);
        assert.ok(
          seen.slice(previous).includes(grant.revision),
          JSON.stringify({ grant: grant.revision, seen, wire, brokerError }),
        );
      } finally {
        await worker.close();
        isolated.close();
      }
    }
    const old = projection({
      id: "old",
      result: JSON.stringify({ output: "legacy output" }),
      state: "completed",
    });
    assert.equal(Object.hasOwn(old, "measurement"), false);
  } finally {
    await engine?.close();
    ledger?.close();
    globalThis.fetch = fetchNative;
    childProcess.spawn = spawn;
    syncBuiltinESMExports();
    if (proc && proc.exitCode === null) {
      const exit = once(proc, "exit");
      proc.kill();
      await exit;
    }
    for (const server of servers)
      await new Promise((resolve) => server.close(resolve));
    rmSync(dir, { recursive: true, force: true });
  }
});
