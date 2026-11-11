// Verifies the normal single-command entry point and its detached daemon lifecycle.
const [binary, socket, work] = Bun.argv.slice(2);
const gateways = new Set<ReturnType<typeof Bun.spawn>>();
let daemonPID: number | undefined;

function assert(value: unknown, message: string) {
  if (!value) throw new Error(message);
}

function daemonOwner() {
  const lock = socket.slice(0, socket.lastIndexOf("/")) + "/daemon.lock";
  const result = Bun.spawnSync(["/usr/sbin/lsof", "-t", lock]);
  const ids = new TextDecoder().decode(result.stdout).trim().split(/\s+/).filter(Boolean);
  assert(ids.length === 1, "expected one daemon holding the lifetime lock");
  return Number(ids[0]);
}

async function startGateway() {
  const probe = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response() });
  const address = `127.0.0.1:${probe.port}`;
  await probe.stop(true);
  const process = Bun.spawn([binary, "-listen", address, "-session-socket", socket, "-workspace", work], {
    stdout: "ignore", stderr: "inherit",
  });
  gateways.add(process);
  const origin = `http://${address}`;
  for (let attempt = 0; attempt < 100; attempt++) {
    const response = await fetch(origin + "/api/sessions").catch(() => null);
    if (response?.ok) {
      daemonPID = daemonOwner();
      return { process, origin, sessions: (await response.json()).sessions as { session_id: string }[] };
    }
    if (process.exitCode !== null) throw new Error("gateway exited during automatic startup");
    await Bun.sleep(100);
  }
  throw new Error("gateway did not automatically start its daemon");
}

async function stopGateway(process: ReturnType<typeof Bun.spawn>) {
  process.kill();
  await process.exited;
  gateways.delete(process);
}

try {
  const [first, second] = await Promise.all([startGateway(), startGateway()]);
  assert(first.sessions.length === 1 && second.sessions.length === 1, "default session duplicated");
  const session = first.sessions[0].session_id;
  assert(second.sessions[0].session_id === session, "gateways connected to different default sessions");
  const view = await (await fetch(first.origin + "/api/session?" + new URLSearchParams({ session }))).json();
  assert(view.windows.length === 1 && view.windows[0].layers[0].blocks.length === 1, "default shell pane missing");

  await stopGateway(first.process);
  await stopGateway(second.process);
  const restarted = await startGateway();
  assert(restarted.sessions[0].session_id === session, "gateway restart replaced the session daemon");
  await stopGateway(restarted.process);

  process.kill(daemonPID!, "SIGKILL");
  for (let attempt = 0; attempt < 50; attempt++) {
    try { process.kill(daemonPID!, 0); } catch { break; }
    await Bun.sleep(50);
  }
  daemonPID = undefined;
  const recovered = await startGateway();
  assert(recovered.sessions.length === 1 && recovered.sessions[0].session_id !== session,
    "stale socket recovery did not create one fresh workspace session");
  console.log("Automatic startup passed: one daemon, one default shell, gateway restart, and stale socket recovery.");
} finally {
  for (const gateway of gateways) await stopGateway(gateway);
  if (daemonPID !== undefined) process.kill(daemonPID, "SIGTERM");
}
