// Exercises the native wire protocol without the Superlogical server.
function connect(address: string, route: string, protocol: string) {
  const ws = new WebSocket(`ws://${address}/api/${route}`, protocol);
  ws.binaryType = "arraybuffer";
  const queue: (string | ArrayBuffer)[] = [];
  let wake: (() => void) | undefined;
  let ended = false;
  ws.onmessage = (event) => { queue.push(event.data); wake?.(); };
  ws.onclose = () => { ended = true; wake?.(); };
  return {
    ws,
    open: new Promise<void>((resolve, reject) => {
      ws.onopen = () => resolve();
      ws.onerror = () => reject(new Error(`could not open Rex ${route} WebSocket`));
    }),
    async next() {
      while (!queue.length && !ended) {
        await new Promise<void>((resolve) => { wake = resolve; });
      }
      if (!queue.length) throw new Error(`Rex ${route} WebSocket closed`);
      return queue.shift()!;
    },
  };
}

function check(value: unknown, message: string) {
  if (!value) throw new Error(message);
}

export async function smokeNative(address: string, session: string, block: string) {
  const crossOrigin = await fetch(`http://${address}/api/control`, {
    headers: { Origin: "http://invalid.example" },
  });
  check(crossOrigin.status === 403, "native listener accepted a browser origin");
  const control = connect(address, "control", "rex.control.v1");
  let data: ReturnType<typeof connect> | undefined;
  const timer = setTimeout(() => {
    control.ws.close();
    data?.ws.close();
  }, 8000);
  try {
    await control.open;
    control.ws.send(JSON.stringify({ type: "hello", api_revision: 12, api_min_revision: 12 }));
    const welcome = JSON.parse(await control.next() as string);
    check(welcome.type === "welcome" && welcome.api_revision === 12, "wrong control welcome");

    let id = 0;
    const rpc = async (method: string, payload: object) => {
      const requestID = String(++id);
      control.ws.send(JSON.stringify({ type: "request", id: requestID, method, payload }));
      for (;;) {
        const reply = JSON.parse(await control.next() as string);
        if (reply.id !== requestID) continue;
        if (reply.error) throw new Error(`${method}: ${reply.error.code} ${reply.error.message}`);
        return reply.payload;
      }
    };

    const bootstrap = await rpc("session.bootstrap", {
      event_scope: "session", attach_session_ids: [session],
    });
    check(bootstrap.attached_sessions[0].view.session_id === session, "missing attached workspace");
    const target = { session_id: session, block_id: block };
    const ticket = await rpc("data.connect", { ...target, data_ref_id: "terminal", subprotocol: "rex.data.terminal.v1" });
    data = connect(address, "data", "rex.data.terminal.v1");
    await data.open;
    data.ws.send(Buffer.concat([Buffer.from([1]), Buffer.from(ticket.ticket, "base64url")]));
    const dataWelcome = new Uint8Array(await data.next() as ArrayBuffer);
    check(dataWelcome.length === 32 && dataWelcome[0] === 0x81, "wrong terminal stream welcome");
    const decoder = new TextDecoder();
    check(decoder.decode(await data.next() as ArrayBuffer).startsWith("GHOSTSNP"), "missing terminal bootstrap snapshot");
    check(decoder.decode(await data.next() as ArrayBuffer).includes('"type":"resize"'), "missing initial terminal size");

    await rpc("block.method", { ...target, method: "resize", args: { columns: 93, rows: 31 } });
    const mode = await rpc("block.method", { ...target, method: "input_mode" });
    check(typeof mode.echo === "boolean" && mode.signals, "terminal input mode unavailable");
    await rpc("block.event", { ...target, event: { type: "paste", payload: {
      text: "printf 'native-%s-%s\\n' OK \"$(stty size)\"",
    } } });
    await rpc("client.key", { ...target, key: { key: "enter", action: "press" } });
    let output = "";
    while (!output.includes("native-OK-31 93")) {
      output += decoder.decode(await data.next() as ArrayBuffer, { stream: true });
    }

    const split = await rpc("session.new_split", {
      ...target, anchor_id: block, direction: "vertical", side: "after", focus: true,
      layout: { block: { flavor: "com.superlogical.terminal.shell" } },
    });
    check(split.block_ids.length === 1 && split.anchor_id === block, "native split failed");
    return split.block_ids[0] as string;
  } finally {
    clearTimeout(timer);
    data?.ws.close();
    control.ws.close();
  }
}
