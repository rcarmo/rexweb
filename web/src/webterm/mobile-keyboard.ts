import { inputFrame, visibleFrame } from "../viewport";

/** One mobile input and toolbar per document, routed to the active terminal. */
/** Detect if running on a mobile/touch device */
export function isMobileDevice(): boolean {
  return (
    /Android|webOS|iPhone|iPad|iPod|BlackBerry|IEMobile|Opera Mini/i.test(
      navigator.userAgent
    ) ||
    ("ontouchstart" in window && navigator.maxTouchPoints > 0)
  );
}

const SHIFT_KEY_MAP: Record<string, string> = {
  "`": "~",
  "1": "!",
  "2": "@",
  "3": "#",
  "4": "$",
  "5": "%",
  "6": "^",
  "7": "&",
  "8": "*",
  "9": "(",
  "0": ")",
  "-": "_",
  "=": "+",
  "[": "{",
  "]": "}",
  "\\": "|",
  ";": ":",
  "'": "\"",
  ",": "<",
  ".": ">",
  "/": "?",
};

const CTRL_KEY_MAP: Record<string, string> = {
  "2": "@",
  "3": "[",
  "4": "\\",
  "5": "]",
  "6": "^",
  "7": "_",
  "8": "?",
};

const FN_NORMAL_KEYS = [
  "\x1bOP",
  "\x1bOQ",
  "\x1bOR",
  "\x1bOS",
  "\x1b[15~",
  "\x1b[17~",
  "\x1b[18~",
  "\x1b[19~",
  "\x1b[20~",
  "\x1b[21~",
];

const FN_SHIFT_KEYS = [
  "\x1b[23~",
  "\x1b[24~",
  "\x1b[25~",
  "\x1b[26~",
  "\x1b[28~",
  "\x1b[29~",
  "\x1b[31~",
  "\x1b[32~",
  "\x1b[33~",
  "\x1b[34~",
];

function applyShiftModifier(key: string): string {
  if (key.length !== 1) {
    return key;
  }
  if (key >= "a" && key <= "z") {
    return key.toUpperCase();
  }
  return SHIFT_KEY_MAP[key] ?? key;
}

function applyCtrlModifier(key: string): string {
  if (key.length !== 1) {
    return key;
  }
  const mapped = CTRL_KEY_MAP[key] ?? key;
  if (mapped === "?") {
    return "\x7f";
  }
  const code = mapped.toUpperCase().charCodeAt(0);
  if (code >= 64 && code <= 95) {
    return String.fromCharCode(code - 64);
  }
  return key;
}

function applyFnModifier(key: string, useShift: boolean): string | null {
  if (key.length !== 1) {
    return null;
  }
  const index = "1234567890".indexOf(key);
  if (index < 0) {
    return null;
  }
  return useShift ? FN_SHIFT_KEYS[index] : FN_NORMAL_KEYS[index];
}

function applyAltModifier(text: string): string {
  if (!text || text.startsWith("\x1b")) {
    return text;
  }
  return `\x1b${text}`;
}

function applyModifiers(
  text: string,
  useShift: boolean,
  useCtrl: boolean,
  useAlt: boolean,
  useFn: boolean
): string {
  if (text.length !== 1) {
    return text;
  }
  if (useFn) {
    const fnApplied = applyFnModifier(text, useShift);
    if (fnApplied) {
      return useAlt ? applyAltModifier(fnApplied) : fnApplied;
    }
  }
  if (useCtrl) {
    const ctrlApplied = applyCtrlModifier(text);
    if (ctrlApplied !== text) {
      return useAlt ? applyAltModifier(ctrlApplied) : ctrlApplied;
    }
  }
  if (useShift) {
    const shifted = applyShiftModifier(text);
    return useAlt ? applyAltModifier(shifted) : shifted;
  }
  return useAlt ? applyAltModifier(text) : text;
}


type Anchor = { left: number; top: number; height: number };
type Target = {
  element: HTMLElement;
  send: (text: string) => void;
  paste: (text: string) => void;
  anchor: () => Anchor;
  selection: () => string;
  copy: () => boolean;
  upload: (files:File[])=>void;
};
export type KeyboardBinding = {
  focus(): void;
  updatePosition(): void;
  prepareCopy(): void;
  dispose(): void;
};
let shared: MobileKeyboard | undefined;

export function bindMobileKeyboard(
  element: HTMLElement,
  send: (text: string) => void,
  paste: (text: string) => void,
  anchor: () => Anchor,
  selection: () => string,
  copy: () => boolean,
  upload: (files:File[])=>void,
): KeyboardBinding {
  shared ??= new MobileKeyboard();
  return shared.bind({ element, send, paste, anchor, selection, copy, upload });
}

class MobileKeyboard {
  private targets = new Set<Target>();
  private active?: Target;
  private composing = false;
  private positionFrame?: number;
  private lastComposition?: { text: string; at: number };
  private lastPaste?: { text: string; source: string; at: number };
  private mobileInput: HTMLTextAreaElement | null = null;
  private mobileKeybar: HTMLElement | null = null;
  private mobileKeybarStyle: HTMLStyleElement | null = null;
  private ctrlActive = false;
  private altActive = false;
  private shiftActive = false;
  private fnActive = false;
  private pendingCtrl = false;
  private pendingAlt = false;
  private pendingShift = false;
  private pendingFn = false;
  private boundHandlers: {
    target: EventTarget;
    type: string;
    handler: EventListener;
    options?: boolean | AddEventListenerOptions;
  }[] = [];

  constructor() {
    this.setupMobileKeyboard();
    this.setupMobileKeybar();
    this.addTrackedListener(window, "resize", () => this.schedulePosition());
    this.addTrackedListener(document, "scroll", () => this.schedulePosition(), {
      capture: true,
      passive: true,
    });
    if (window.visualViewport) {
      this.addTrackedListener(window.visualViewport, "resize", () =>
        this.schedulePosition(),
      );
      this.addTrackedListener(window.visualViewport, "scroll", () =>
        this.schedulePosition(),
      );
    }
  }

  bind(target: Target): KeyboardBinding {
    this.targets.add(target);
    if (!this.active) this.active = target;
    const focus = () => {
      if (!this.targets.has(target)) return;
      if (this.active !== target) {
        this.deactivateModifiers();
        this.composing = false;
        this.lastPaste = this.lastComposition = undefined;
        if (this.mobileInput) this.mobileInput.value = "";
      }
      this.active = target;
      this.position();
      this.focusMobileInput();
    };
    target.element.addEventListener("touchend", focus, { passive: true });
    target.element.addEventListener("click", focus);
    return {
      focus,
      updatePosition: () => {
        if (this.active === target) this.position();
      },
      prepareCopy: () => {
        focus();
        this.prepareCopy();
      },
      dispose: () => {
        if (!this.targets.delete(target)) return;
        target.element.removeEventListener("touchend", focus);
        target.element.removeEventListener("click", focus);
        if (this.active === target) {
          this.active = this.targets.values().next().value;
          this.deactivateModifiers();
          this.composing = false;
          this.lastPaste = this.lastComposition = undefined;
          if (this.mobileInput) this.mobileInput.value = "";
        }
        if (!this.targets.size) {
          if (this.positionFrame !== undefined) window.cancelAnimationFrame(this.positionFrame);
          this.positionFrame = undefined;
          for (const { target, type, handler, options } of this.boundHandlers)
            target.removeEventListener(type, handler, options);
          this.mobileInput?.remove();
          this.mobileKeybar?.remove();
          this.mobileKeybarStyle?.remove();
          shared = undefined;
        }
      },
    };
  }

  private schedulePosition(): void {
    // The workspace applies its viewport frame first; read the cursor afterwards.
    this.positionFrame ??= window.requestAnimationFrame(() => {
      this.positionFrame = undefined;
      this.position();
    });
  }

  private position(): void {
    if (!this.active || !this.mobileInput || !this.active.element.isConnected)
      return;
    const anchor = this.active.anchor();
    if (
      !Number.isFinite(anchor.left) ||
      !Number.isFinite(anchor.top) ||
      !Number.isFinite(anchor.height)
    )
      return;
    const bounds = inputFrame(anchor, visibleFrame(window.innerWidth, window.innerHeight, window.visualViewport));
    if (!bounds) return;
    this.mobileInput.style.left = `${bounds.left}px`;
    this.mobileInput.style.top = `${bounds.top}px`;
    this.mobileInput.style.height = `${bounds.height}px`;
  }

  private prepareCopy(): void {
    const text = this.active?.selection();
    if (!text || !this.mobileInput) return;
    this.mobileInput.value = text;
    this.focusMobileInput();
    this.mobileInput.select();
  }

  private sendStdin(text: string): void {
    this.active?.send(text);
  }
  private addTrackedListener(
    target: EventTarget,
    type: string,
    handler: EventListener,
    options?: boolean | AddEventListenerOptions,
  ): void {
    target.addEventListener(type, handler, options);
    this.boundHandlers.push({ target, type, handler, options });
  }

  /** Setup mobile keyboard input via hidden textarea */
  private setupMobileKeyboard(): void {
    // Create hidden textarea for mobile keyboard input
    const textarea = document.createElement("textarea");
    textarea.setAttribute("autocapitalize", "off");
    textarea.setAttribute("autocomplete", "off");
    textarea.setAttribute("autocorrect", "off");
    textarea.setAttribute("spellcheck", "false");
    textarea.setAttribute("inputmode", "text");
    textarea.setAttribute("enterkeyhint", "send");
    textarea.setAttribute("wrap", "off");
    // iOS requires the element to be "visible" and interactive for keyboard
    // Use opacity near-zero but not zero, and keep it in the visible area
    textarea.dataset.mobileKeyboardInput = "";
    textarea.setAttribute("aria-label", "Terminal keyboard input");
    textarea.style.cssText = `
      position: fixed;
      left: 0;
      top: 0;
      width: 1px;
      height: 1px;
      opacity: 0.01;
      z-index: 1;
      color: transparent;
      background: transparent;
      border: none;
      outline: none;
      resize: none;
      font-size: 16px;
      line-height: 16px;
      padding: 0;
      margin: 0;
      overflow: hidden;
      caret-color: transparent;
      pointer-events: none;
    `;
    // Font size 16px prevents iOS auto-zoom on focus

    document.body.appendChild(textarea);
    this.mobileInput = textarea;
    textarea.addEventListener("copy", (event) => {
      const text = this.active?.selection();
      if (text && event.clipboardData) {
        event.preventDefault();
        event.stopPropagation();
        event.clipboardData.setData("text/plain", text);
        textarea.value = "";
      }
    });
    textarea.addEventListener("blur", () => {
      if (!this.composing) textarea.value = "";
    });

    const applyMobileModifiers = (text: string): string =>
      applyModifiers(
        text,
        this.shiftActive || this.pendingShift,
        this.ctrlActive || this.pendingCtrl,
        this.altActive || this.pendingAlt,
        this.fnActive || this.pendingFn,
      );

    const handleMobileInput = (text: string, e?: Event) => {
      if (e) {
        e.preventDefault();
        e.stopPropagation();
      }
      if (!text) {
        return;
      }
      this.sendStdin(applyMobileModifiers(text));
      textarea.value = "";
      this.deactivateModifiers();
      this.pendingCtrl = false;
      this.pendingAlt = false;
      this.pendingShift = false;
      this.pendingFn = false;
    };

    const paste = (text: string, source: string) => {
      const previous = this.lastPaste;
      if (
        text &&
        !(
          previous &&
          previous.text === text &&
          previous.source !== source &&
          performance.now() - previous.at < 100
        )
      ) {
        this.active?.paste(text);
        this.lastPaste = { text, source, at: performance.now() };
      }
      textarea.value = "";
      this.deactivateModifiers();
    };
    textarea.addEventListener("paste", (e) => {
      if (!e.clipboardData) return;
      const files=Array.from(e.clipboardData.files);
      if(files.length){e.preventDefault();e.stopPropagation();this.active?.upload(files);textarea.value='';return;}
      e.preventDefault();
      e.stopPropagation();
      paste(e.clipboardData.getData("text/plain"), "paste");
    });
    textarea.addEventListener("compositionstart", () => {
      this.composing = true;
    });
    textarea.addEventListener("compositionend", (e) => {
      this.composing = false;
      handleMobileInput(e.data);
      this.lastComposition = { text: e.data, at: performance.now() };
    });

    // Browser paste and IME sequences can each arrive through several events.
    textarea.addEventListener("beforeinput", (e) => {
      if (this.composing || e.isComposing) return;
      if (e.inputType === "insertFromPaste") {
        if (e.data !== null) {
          e.preventDefault();
          e.stopPropagation();
          paste(e.data, "beforeinput");
        }
        return;
      }
      if (
        this.lastComposition &&
        e.data === this.lastComposition.text &&
        performance.now() - this.lastComposition.at < 100
      ) {
        this.lastComposition = undefined;
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      if (
        (e.inputType === "insertText" ||
          e.inputType === "insertReplacementText") &&
        e.data
      ) {
        handleMobileInput(e.data, e);
        return;
      }

      let seq: string | null = null;
      switch (e.inputType) {
        case "insertLineBreak": // Enter key
          seq = "\r";
          break;
        case "deleteContentBackward": // Backspace
          seq = "\x7f";
          break;
        case "deleteContentForward": // Delete
          seq = "\x1b[3~";
          break;
      }
      if (seq) {
        handleMobileInput(seq, e);
      }
    });

    // Handle input from mobile keyboard (regular text only, special keys handled above)
    textarea.addEventListener("input", (event) => {
      const e = event as InputEvent;
      if (this.composing || e.isComposing) return;
      if (e.inputType === "insertFromPaste") paste(textarea.value, "input");
      else handleMobileInput(textarea.value);
    });

    // Handle special navigation keys via keydown (not covered by beforeinput)
    // Check both physical keyboard modifiers (e.ctrlKey/e.shiftKey) and mobile keybar flags
    textarea.addEventListener("keydown", (e) => {
      if (
        (e.ctrlKey || e.metaKey) &&
        e.key.toLowerCase() === "c" &&
        this.active?.selection()
      ) {
        if (this.active.copy()) {
          e.preventDefault();
          e.stopPropagation();
          textarea.value = "";
        }
        return;
      }
      if (!this.composing) textarea.value = "";
      if (this.composing || e.isComposing || e.keyCode === 229) return;
      const isCtrl = e.ctrlKey || this.ctrlActive;
      const isShift = e.shiftKey || this.shiftActive;
      const isAlt = e.altKey || this.altActive;
      const isFn = this.fnActive;

      // Handle Ctrl+key combinations (these don't fire input events)
      if (isCtrl && e.key.length === 1 && !e.altKey && !e.metaKey) {
        const ctrlApplied = applyCtrlModifier(e.key);
        if (ctrlApplied !== e.key) {
          e.preventDefault();
          e.stopPropagation();
          const toSend = isAlt ? applyAltModifier(ctrlApplied) : ctrlApplied;
          this.sendStdin(toSend); // Ctrl+A=0x01, Ctrl+C=0x03, etc.
          this.deactivateModifiers(); // Clear modifiers after physical Ctrl+key
          return;
        }
      }
      if (isFn && e.key.length === 1 && !e.ctrlKey && !e.metaKey) {
        const fnApplied = applyFnModifier(e.key, isShift);
        if (fnApplied) {
          e.preventDefault();
          e.stopPropagation();
          this.sendStdin(fnApplied);
          this.deactivateModifiers();
          return;
        }
      }

      let seq: string | null = null;
      switch (e.key) {
        case "Escape":
          seq = "\x1b";
          break;
        case "ArrowUp":
        case "ArrowDown":
        case "ArrowRight":
        case "ArrowLeft": {
          const dir =
            e.key === "ArrowUp"
              ? "A"
              : e.key === "ArrowDown"
                ? "B"
                : e.key === "ArrowRight"
                  ? "C"
                  : "D";
          if (isCtrl && isShift) {
            seq = `\x1b[1;6${dir}`;
          } else if (isCtrl) {
            seq = `\x1b[1;5${dir}`;
          } else if (isShift) {
            seq = `\x1b[1;2${dir}`;
          } else {
            seq = `\x1b[${dir}`;
          }
          break;
        }
        case "Tab":
          if (isShift) {
            seq = "\x1b[Z"; // Back-tab
          } else {
            seq = "\t";
          }
          e.preventDefault();
          break;
      }
      if (seq) {
        e.preventDefault();
        e.stopPropagation();
        this.sendStdin(isAlt ? applyAltModifier(seq) : seq);
        // Always clear modifiers after any key
        this.deactivateModifiers();
      }
    });

    // Apply keybar modifiers to physical keyboard input even when the textarea isn't focused.
    this.addTrackedListener(
      document,
      "keydown",
      ((event: KeyboardEvent) => {
        if (
          !this.active ||
          !(
            event.target === this.mobileInput ||
            this.active.element.contains(event.target as Node)
          ) ||
          (!this.ctrlActive &&
            !this.shiftActive &&
            !this.altActive &&
            !this.fnActive)
        ) {
          return;
        }
        if (event.target === this.mobileInput) {
          return;
        }

        const useCtrl = this.ctrlActive;
        const useShift = this.shiftActive;
        const useAlt = this.altActive;
        const useFn = this.fnActive;
        let handled = false;

        if (event.key.length === 1 && !event.altKey && !event.metaKey) {
          const toSend = applyModifiers(
            event.key,
            useShift,
            useCtrl,
            useAlt,
            useFn,
          );
          event.preventDefault();
          event.stopPropagation();
          this.sendStdin(toSend);
          handled = true;
        } else {
          let seq: string | null = null;
          switch (event.key) {
            case "Escape":
              seq = "\x1b";
              break;
            case "ArrowUp":
            case "ArrowDown":
            case "ArrowRight":
            case "ArrowLeft": {
              const dir =
                event.key === "ArrowUp"
                  ? "A"
                  : event.key === "ArrowDown"
                    ? "B"
                    : event.key === "ArrowRight"
                      ? "C"
                      : "D";
              if (useCtrl && useShift) {
                seq = `\x1b[1;6${dir}`;
              } else if (useCtrl) {
                seq = `\x1b[1;5${dir}`;
              } else if (useShift) {
                seq = `\x1b[1;2${dir}`;
              } else {
                seq = `\x1b[${dir}`;
              }
              break;
            }
            case "Tab":
              if (useShift) {
                seq = "\x1b[Z";
              } else {
                seq = "\t";
              }
              break;
          }

          if (seq) {
            event.preventDefault();
            event.stopPropagation();
            this.sendStdin(useAlt ? applyAltModifier(seq) : seq);
            handled = true;
          }
        }

        if (handled) {
          this.deactivateModifiers();
        }
      }) as EventListener,
      { capture: true },
    );
  }

  /** Setup draggable mobile extended keyboard bar */
  private setupMobileKeybar(): void {
    const keybar = document.createElement("div");
    keybar.className = "mobile-keybar";
    keybar.setAttribute("role", "toolbar");
    keybar.setAttribute("aria-label", "Terminal keyboard helper");
    keybar.innerHTML = `
      <button class="keybar-drag" title="Drag to move">⋮⋮</button>
      <button data-key="\\x1b" title="Escape">Esc</button>
      <button data-modifier="ctrl" title="Ctrl modifier">Ctrl</button>
      <button data-modifier="alt" title="Alt modifier">Alt</button>
      <button data-modifier="fn" title="Fn modifier">Fn</button>
      <button data-key="\\x09" title="Tab">Tab</button>
      <button data-modifier="shift" title="Shift modifier">⇧</button>
      <button data-key="\\x1b[A" title="Up">↑</button>
      <button data-key="\\x1b[B" title="Down">↓</button>
      <button data-key="\\x1b[D" title="Left">←</button>
      <button data-key="\\x1b[C" title="Right">→</button>
      <button data-key="\\x0d" title="Return" class="keybar-return">⏎</button>
    `;

    // Inject styles
    const style = document.createElement("style");
    style.dataset.mobileKeyboardStyle = "";
    style.textContent = `
      .mobile-keybar {
        position: fixed;
        bottom: 80px;
        right: 0;
        display: grid;
        grid-template-columns: repeat(6, auto);
        gap: 4px;
        padding: 6px;
        background: rgba(40, 40, 40, 0.95);
        border-radius: 8px 0 0 8px;
        box-shadow: 0 2px 10px rgba(0,0,0,0.3);
        z-index: 10000;
        touch-action: none;
        user-select: none;
        -webkit-user-select: none;
      }
      .mobile-keybar button {
        min-width: 36px;
        height: 32px;
        padding: 0 8px;
        border: 1px solid #555;
        border-radius: 4px;
        background: #333;
        color: #eee;
        font-size: 13px;
        font-family: system-ui, sans-serif;
        cursor: pointer;
        touch-action: manipulation;
      }
      .mobile-keybar button:active {
        background: #555;
      }
      .mobile-keybar button.active {
        background: #0066cc;
        border-color: #0088ff;
      }
      .mobile-keybar .keybar-drag {
        min-width: 24px;
        padding: 0 4px;
        cursor: grab;
        color: #888;
      }
      .mobile-keybar .keybar-drag:active {
        cursor: grabbing;
      }
      .mobile-keybar .keybar-return {
        grid-column: 6;
        grid-row: 2;
      }
    `;
    document.head.appendChild(style);
    this.mobileKeybarStyle = style;
    document.body.appendChild(keybar);
    this.mobileKeybar = keybar;

    // Handle key button presses
    keybar.querySelectorAll("button[data-key]").forEach((btn) => {
      btn.addEventListener("pointerdown", (e) => {
        e.preventDefault();
        let key = (btn as HTMLElement).dataset.key || "";
        // Unescape the key sequences
        key = key.replace(/\\x([0-9a-fA-F]{2})/g, (_, hex) =>
          String.fromCharCode(parseInt(hex, 16)),
        );
        key = key.replace(/\\x1b/g, "\x1b");

        const useShift = this.shiftActive;
        const useCtrl = this.ctrlActive;
        const useAlt = this.altActive;
        const useFn = this.fnActive;
        const useShiftForFn = useShift || this.pendingShift;
        // Handle Shift+Tab -> Back-Tab (CSI Z)
        if (useShift && key === "\x09") {
          key = "\x1b[Z";
        }
        // Handle Ctrl+Shift+Arrow keys (CSI 1;6 X)
        else if (
          useCtrl &&
          useShift &&
          key.startsWith("\x1b[") &&
          key.length === 3
        ) {
          const dir = key[2];
          key = `\x1b[1;6${dir}`;
        }
        // Handle Shift+Arrow keys (CSI 1;2 X)
        else if (useShift && key.startsWith("\x1b[") && key.length === 3) {
          const dir = key[2]; // A, B, C, or D
          key = `\x1b[1;2${dir}`;
        }
        // Handle Ctrl+Arrow keys (CSI 1;5 X)
        else if (useCtrl && key.startsWith("\x1b[") && key.length === 3) {
          const dir = key[2];
          key = `\x1b[1;5${dir}`;
        }
        if (useFn && key.length === 1) {
          const fnApplied = applyFnModifier(key, useShiftForFn);
          if (fnApplied) {
            key = fnApplied;
          }
        }
        if (key.length === 1) {
          key = applyModifiers(key, useShift, useCtrl, useAlt, useFn);
        } else if (useAlt) {
          key = applyAltModifier(key);
        }

        this.sendStdin(key);
        this.deactivateModifiers();
        this.focusMobileInput();
      });
    });

    // Handle modifier toggles
    keybar.querySelectorAll("button[data-modifier]").forEach((btn) => {
      btn.addEventListener("pointerdown", (e) => {
        e.preventDefault();
        const modifier = (btn as HTMLElement).dataset.modifier;
        if (modifier === "ctrl") {
          this.ctrlActive = !this.ctrlActive;
          this.pendingCtrl = this.ctrlActive;
          btn.classList.toggle("active", this.ctrlActive);
        } else if (modifier === "alt") {
          this.altActive = !this.altActive;
          this.pendingAlt = this.altActive;
          btn.classList.toggle("active", this.altActive);
        } else if (modifier === "shift") {
          this.shiftActive = !this.shiftActive;
          this.pendingShift = this.shiftActive;
          btn.classList.toggle("active", this.shiftActive);
        } else if (modifier === "fn") {
          this.fnActive = !this.fnActive;
          this.pendingFn = this.fnActive;
          btn.classList.toggle("active", this.fnActive);
        }
        this.focusMobileInput();
      });
    });

    // Setup drag functionality
    this.setupKeybarDrag(keybar);
  }

  /** Make the keybar draggable */
  private setupKeybarDrag(keybar: HTMLElement): void {
    const dragHandle = keybar.querySelector(".keybar-drag") as HTMLElement;
    if (!dragHandle) return;

    let isDragging = false;
    let startX = 0;
    let startY = 0;
    let startRight = 0;
    let startBottom = 0;

    const onTouchStart = (e: TouchEvent) => {
      if (e.touches.length !== 1) return;
      isDragging = true;
      const touch = e.touches[0];
      startX = touch.clientX;
      startY = touch.clientY;

      const rect = keybar.getBoundingClientRect();
      startRight = window.innerWidth - rect.right;
      startBottom = window.innerHeight - rect.bottom;

      e.preventDefault();
    };

    const onTouchMove = (e: TouchEvent) => {
      if (!isDragging || e.touches.length !== 1) return;
      const touch = e.touches[0];
      const deltaX = startX - touch.clientX;
      const deltaY = startY - touch.clientY;

      const newRight = Math.max(
        0,
        Math.min(window.innerWidth - 100, startRight + deltaX),
      );
      const newBottom = Math.max(
        0,
        Math.min(window.innerHeight - 50, startBottom + deltaY),
      );

      keybar.style.right = `${newRight}px`;
      keybar.style.bottom = `${newBottom}px`;

      e.preventDefault();
    };

    const onTouchEnd = () => {
      isDragging = false;
    };

    this.addTrackedListener(
      dragHandle,
      "touchstart",
      onTouchStart as EventListener,
      { passive: false },
    );
    this.addTrackedListener(
      document,
      "touchmove",
      onTouchMove as EventListener,
      { passive: false },
    );
    this.addTrackedListener(document, "touchend", onTouchEnd as EventListener);
  }

  /** Deactivate all modifiers */
  private deactivateModifiers(): void {
    this.ctrlActive = false;
    this.altActive = false;
    this.shiftActive = false;
    this.fnActive = false;
    this.pendingCtrl = false;
    this.pendingAlt = false;
    this.pendingShift = false;
    this.pendingFn = false;
    this.mobileKeybar
      ?.querySelectorAll("button[data-modifier]")
      .forEach((btn) => {
        btn.classList.remove("active");
      });
  }

  /** Focus the mobile input to show keyboard */
  private focusMobileInput(): void {
    // For programmatic focus (not from user gesture), this may not show keyboard on iOS
    this.position();
    this.mobileInput?.focus({ preventScroll: true });
  }
}
