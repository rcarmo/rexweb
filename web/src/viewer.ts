/** Coordinate one terminal viewer across same-origin tabs/iframes. The server
 * remains authoritative and keeps its single-connection ownership checks. */
let documentIntent = Date.now();
for (const type of ['pointerdown', 'keydown']) document.addEventListener(type, event => {
  if (event.isTrusted) documentIntent = Math.max(Date.now(), documentIntent + 1);
}, {capture: true, passive: true});

export class TerminalViewer {
  private channel?: BroadcastChannel;
  private id = Array.from(crypto.getRandomValues(new Uint32Array(4)), n => n.toString(16)).join('-');
  private best = { id: this.id, at: 0 };
  private timer?: number;
  private state: 'claiming' | 'active' | 'paused' | 'closed' = 'claiming';
  constructor(key: string, private acquired: () => void, private lost: () => void) {
    if (typeof BroadcastChannel !== 'undefined') {
      this.channel = new BroadcastChannel(`rexweb:viewer:${key}`);
      this.channel.onmessage = ({ data }) => {
        if (this.state === 'closed') return;
        if (data?.type !== 'claim' || typeof data.id !== 'string' ||
          !Number.isSafeInteger(data.at) || data.at < 0 || data.id === this.id) return;
        const newer = data.at > this.best.at || (data.at === this.best.at && data.id > this.best.id);
        if (newer) {
          this.best = { id: data.id, at: data.at };
          if (this.state !== 'paused') {
            clearTimeout(this.timer); this.state = 'paused'; this.lost();
          }
        } else if (this.best.id === this.id) this.announce();
      };
    }
    this.claim(false);
  }
  get active() { return this.state === 'active'; }
  private announce() { this.channel?.postMessage({ type: 'claim', ...this.best }); }
  private claim(foreground: boolean) {
    clearTimeout(this.timer);
    // Layout events can remount a pane in background documents. Such a remount
    // keeps that document's previous user intent, rather than stealing a newer
    // foreground viewer simply because its WASM finished initializing later.
    if (foreground) documentIntent = Math.max(Date.now(), documentIntent, this.best.at + 1);
    this.best = { id: this.id, at: documentIntent };
    this.state = 'claiming'; this.announce();
    // Give the previous browser socket time to close through an embedding proxy.
    this.timer = window.setTimeout(() => {
      if (this.state !== 'claiming') return;
      this.state = 'active'; this.acquired();
    }, 150);
  }
  resume() {
    if (this.state === 'paused' || this.state === 'claiming') this.claim(true);
  }
  dispose() {
    this.state = 'closed'; clearTimeout(this.timer); this.channel?.close();
  }
}
