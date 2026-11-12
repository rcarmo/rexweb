import type { Page } from "../web/node_modules/playwright";

/** Force a disconnect during a yielded replay and inspect the next transport's offset. */
export async function checkReplay(page: Page, origin: string) {
  const response = await fetch(origin + "/api/action", {
    method: "POST",
    headers: { Origin: origin, "Content-Type": "application/json" },
    body: JSON.stringify({ op: "create", label: "replay-check" }),
  });
  if (!response.ok) throw new Error("Failed to create replay fixture");
  const {
    session_id: session,
    initial_windows: [first],
  } = await response.json();
  const block = first.block_ids[0];
  try {
    await new Promise<void>((resolve, reject) => {
      const args = new URLSearchParams({ session, block });
      const socket = new WebSocket(
        origin.replace("http:", "ws:") + "/api/terminal?" + args,
        { headers: { Origin: origin } },
      );
      socket.binaryType = "arraybuffer";
      let output = "";
      let failure: Error | undefined;
      const timeout = setTimeout(() => {
        socket.close();
        reject(new Error("Replay fixture timed out"));
      }, 10000);
      socket.onmessage = (event) => {
        if (typeof event.data === "string") {
          if (JSON.parse(event.data)[0] === "ready")
            socket.send(
              JSON.stringify([
                "stdin",
                "i=0; while [ $i -lt 60 ]; do head -c 10000 /dev/zero | tr '\\000' R; sleep 0.01; i=$((i+1)); done; printf '\\nREPLAY-%s\\n' END\r",
              ]),
            );
        } else {
          output += new TextDecoder().decode(event.data);
          if (output.includes("REPLAY-END")) socket.close();
        }
      };
      socket.onerror = () => {
        failure = new Error("Replay fixture websocket failed");
      };
      socket.onclose = () => {
        clearTimeout(timeout);
        if (failure || !output.includes("REPLAY-END"))
          reject(failure ?? new Error("Replay fixture closed early"));
        else resolve();
      };
    });
    await page.waitForTimeout(100);
    await page.addInitScript(
      ({ target }) => {
        if (!location.search.includes("auditReplay=1")) return;
        const native = window.WebSocket;
        const records: any[] = [];
        (window as any).__replayRecords = records;
        let interrupted = false;
        const timer = window.setTimeout.bind(window);
        // Make reconnect race pending parser work instead of waiting its usual second.
        window.setTimeout = ((fn: any, delay?: number, ...args: any[]) =>
          timer(
            fn,
            interrupted && delay === 1000 ? 0 : delay,
            ...args,
          )) as typeof window.setTimeout;
        window.WebSocket = class extends native {
          private record: any;
          constructor(url: string | URL, protocols?: string | string[]) {
            super(url, protocols);
            if (
              new URL(String(url), location.href).searchParams.get(
                "session",
              ) !== target
            )
              return;
            this.record = { url: String(url), start: -1, bytes: 0 };
            records.push(this.record);
            super.addEventListener("message", (event) => {
              if (typeof event.data === "string") {
                const [kind, payload] = JSON.parse(event.data);
                if (kind === "stream") this.record.start = payload.offset;
              } else if (event.data instanceof ArrayBuffer)
                this.record.bytes += event.data.byteLength;
            });
          }
          addEventListener(type: any, listener: any, options?: any): void {
            if (type !== "message" || !this.record) {
              super.addEventListener(type, listener, options);
              return;
            }
            super.addEventListener(
              type,
              (event: any) => {
                const interrupt =
                  !interrupted &&
                  event.data instanceof ArrayBuffer &&
                  event.data.byteLength > 65536;
                const descriptor = Object.getOwnPropertyDescriptor(
                  performance,
                  "now",
                );
                if (interrupt) {
                  let tick = 0;
                  Object.defineProperty(performance, "now", {
                    configurable: true,
                    value: () => ++tick * 10,
                  });
                }
                try {
                  if (typeof listener === "function")
                    listener.call(this, event);
                  else listener.handleEvent(event);
                  if (
                    typeof event.data === "string" &&
                    JSON.parse(event.data)[0] === "ready"
                  )
                    this.record.enabledInsideReadyHandler =
                      !!document.querySelector(".webterm-terminal.-connected");
                } finally {
                  if (interrupt) {
                    if (descriptor)
                      Object.defineProperty(performance, "now", descriptor);
                    else delete (performance as any).now;
                    interrupted = true;
                    this.close();
                  }
                }
              },
              options,
            );
          }
        };
      },
      { target: session },
    );
    await page.goto(
      origin + "/?" + new URLSearchParams({ session, auditReplay: "1" }),
    );
    await page.waitForFunction(
      () => {
        const records = (window as any).__replayRecords;
        return (
          records?.length >= 2 &&
          document.querySelector(".webterm-terminal.-connected")
        );
      },
      undefined,
      { timeout: 15000 },
    );
    const records = await page.evaluate(() => (window as any).__replayRecords);
    const firstRecord = records[0];
    const resumed = Number(new URL(records[1].url).searchParams.get("offset"));
    if (
      firstRecord.start < 0 ||
      firstRecord.bytes < 65536 ||
      resumed !== firstRecord.start + firstRecord.bytes
    ) {
      throw new Error(
        "Reconnect did not wait for parser consumption: " +
          JSON.stringify(records),
      );
    }
    if (
      records.some((record: any) => record.enabledInsideReadyHandler === true)
    )
      throw new Error("Input became ready before the parser barrier");
    console.log(
      "Replay browser check passed: disconnect during yielded parsing resumes at the fully consumed byte offset.",
    );
  } finally {
    await fetch(origin + "/api/action", {
      method: "POST",
      headers: { Origin: origin, "Content-Type": "application/json" },
      body: JSON.stringify({ op: "destroy", session }),
    });
  }
}
