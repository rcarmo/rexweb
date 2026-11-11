import { smokeNative } from "./rex-native";

const [binary, socket, work, canary, rexAddress] = Bun.argv.slice(2);
const listener = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response() });
const address = `127.0.0.1:${listener.port}`;
await listener.stop(true);
const origin = `http://${address}`;

function assert(result: unknown, message: string) {
  if (!result) throw new Error(message);
}

async function call(path: string, data?: unknown) {
  const response = await fetch(origin + path, data === undefined ? undefined : {
    method: "POST",
    headers: { Origin: origin, "Content-Type": "application/json" },
    body: JSON.stringify(data),
  });
  if (!response.ok) throw new Error(`${path} ${response.status} ${await response.text()}`);
  return response.json();
}

function startGateway() {
  return Bun.spawn([binary, "-listen", address, "-session-socket", socket], {
    stdout: "ignore",
    stderr: "inherit",
  });
}

async function waitForGateway() {
  for (let attempt = 0; attempt < 25; attempt++) {
    const response = await fetch(origin).catch(() => null);
    if (response?.ok) {
      assert((await response.text()).includes("Rex Workspace"), "frontend missing");
      return;
    }
    await Bun.sleep(100);
  }
  throw new Error("gateway startup timed out");
}

function checkPane(session: string, block: string, command: string, marker: string, resume?: number) {
  return new Promise<number>((resolve, reject) => {
    const args = new URLSearchParams({ session, block });
    if (resume !== undefined) args.set("offset", String(resume));
    const ws = new WebSocket(`ws://${address}/api/terminal?${args}`, { headers: { Origin: origin } });
    ws.binaryType = "arraybuffer";
    const timer = setTimeout(() => {
      ws.close();
      reject(new Error("pane timed out: " + marker));
    }, 6000);
    let offset = 0;
    let content = "";
    const decoder = new TextDecoder();
    let failure: Error | undefined;
    ws.onmessage = (event) => {
      if (typeof event.data === "string") {
        const [kind, payload] = JSON.parse(event.data);
        if (kind === "stream") {
          offset = payload.offset;
          if (resume !== undefined && (offset !== resume || payload.reset)) {
            failure = new Error("terminal failed to resume at the consumed byte offset");
            ws.close();
          }
        } else if (kind === "ready") {
          ws.send(JSON.stringify(["resize", { width: 91, height: 30 }]));
          ws.send(JSON.stringify(["stdin", command + "\r"]));
        }
        return;
      }
      offset += event.data.byteLength;
      content += decoder.decode(event.data, { stream: true });
      if (content.includes("\x1bc")) {
        failure = new Error("unexpected terminal reset in the PTY stream");
        ws.close();
      } else if (content.includes(marker)) {
        ws.close();
      }
    };
    ws.onerror = () => { failure = new Error("pane connection failed"); };
    ws.onclose = () => {
      clearTimeout(timer);
      if (failure || !content.includes(marker)) reject(failure ?? new Error("pane closed before " + marker));
      else resolve(offset);
    };
  });
}

let gateway = startGateway();
try {
  await waitForGateway();
  const wasm = await fetch(origin + "/static/js/ghostty-vt.wasm");
  assert(wasm.ok && wasm.headers.get("content-type") === "application/wasm", "WASM asset missing");

  const { session_id: session, initial_windows: [first] } =
    await call("/api/action", { op: "create", label: "isolated smoke" });
  const [block] = first.block_ids;
  const list = async () => (await call("/api/sessions")).sessions as { session_id: string }[];
  assert((await list()).some((entry) => entry.session_id === session), "session missing");
  const view = () => call("/api/session?" + new URLSearchParams({ session }));
  const act = (op: string, args: object = {}) => call("/api/action", { op, session, ...args });
  const { block_ids: [right] } = await act("split", { block, direction: "horizontal" });
  await act("split", { block: right, direction: "vertical" });
  await act("resize-split", { before: block, after: right, direction: "horizontal", ratio: .65 });
  const tiled = (await view()).windows[0].layers[0];
  assert(tiled.blocks.length === 3 && tiled.layout.split.ratio === .65, "nested split or resize missing");
  const second = await act("new-window");
  assert((await view()).active_window_id === second.window_id, "new window missing");
  await act("focus-window", { window: first.window_id });
  await act("focus-block", { block });
  assert((await view()).windows[0].focused_block_id === block, "focus missing");
  await act("rename", { label: "renamed" });
  assert((await view()).label === "renamed", "rename missing");

  const quote = (value: string) => "'" + value.replaceAll("'", "'\\''") + "'";
  const isolationChecks = [
    `test "$(pwd -P)" = ${quote(work)}`,
    `test "$HOME" = ${quote(work + "/.home")}`,
    'test -z "$REXWEB_SMOKE_SECRET"',
    'touch "$HOME/allowed"',
    `! /bin/cat ${quote(canary)} >/dev/null 2>&1`,
    '! /bin/cat ./escape >/dev/null 2>&1',
    `! /bin/cat ${quote("/System/Volumes/Data" + canary)} >/dev/null 2>&1`,
    `! /usr/bin/touch ${quote(canary + ".write")} 2>/dev/null`,
    `! /usr/bin/curl -fsS --noproxy '*' --max-time 1 ${quote(origin)} >/dev/null 2>&1`,
    `! /usr/bin/curl -fsS --max-time 1 --unix-socket ${quote(socket)} http://localhost/api/sessions >/dev/null 2>&1`,
  ];
  await checkPane(session, block,
    isolationChecks.join(" && ") + " && printf 'sandbox-%s\\n' enforced",
    "sandbox-enforced");
  assert(await Bun.file(work + "/.home/allowed").exists(), "sandboxed home was not writable");
  assert(!await Bun.file(canary + ".write").exists(), "outside file was written");

  const offset = await checkPane(session, block,
    "export REXWEB_CHECK=alive; printf 'first-%s-%s\\n' \"$REXWEB_CHECK\" \"$(stty size)\"",
    "first-alive-30 91");
  gateway.kill();
  await gateway.exited;
  gateway = startGateway();
  await waitForGateway();
  assert((await view()).windows[0].focused_block_id === block, "layout lost on gateway restart");
  await checkPane(session, block, "printf 'reconnected-%s\\n' \"$REXWEB_CHECK\"", "reconnected-alive", offset);

  const badOrigin = await fetch(origin + "/api/action", {
    method: "POST",
    headers: { Origin: "http://invalid.example", "Content-Type": "application/json" },
    body: JSON.stringify({ op: "destroy", session }),
  });
  assert(badOrigin.status === 403, "cross-origin mutation accepted");
  await act("close-block", { block: right });
  assert((await view()).windows[0].layers[0].blocks.length === 2, "pane did not close");
  const nativePane = await smokeNative(rexAddress, session, block);
  assert((await view()).windows[0].focused_block_id === nativePane, "native split missing in web API");
  await act("destroy");
  assert(!(await list()).some((entry) => entry.session_id === session), "destroyed session listed");
  console.log("Live PTY smoke passed: sandbox isolation, resize, splits, gateway restart, byte-offset resume, origin check, and native Rex wire protocol.");
} finally {
  gateway.kill();
  await gateway.exited;
}
