//! Bounded, headless execution of the Ghostty VT WebAssembly ABI.
//!
//! This deliberately exposes no host filesystem, clocks, environment, sockets, or
//! networking.  Each terminal owns its own module, memory and fuel account.

use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::Deserialize;
use wasmi::{
    AsContext, AsContextMut, Caller, Config, Engine as WasmEngine, Extern, Func, Instance, Linker,
    Memory, Module, Store, Val,
};

const MAX_PAGES: u32 = 1024; // 64 MiB
const MAX_FEED: usize = 64 << 10;
const MAX_CAPTURE: u32 = 4 << 20;
const CALL_FUEL: u64 = 50_000_000;
const CALLBACKS: &[u8] = include_bytes!("../assets/vt/callbacks.wasm");
const EXPORT_CSP: &str =
    "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'";

#[derive(Clone)]
pub struct Engine {
    engine: WasmEngine,
    terminal: Module,
    callbacks: Module,
}

impl Engine {
    pub fn new(wasm: &[u8]) -> Result<Self> {
        let mut config = Config::default();
        config.consume_fuel(true);
        config.wasm_multi_memory(true);
        let engine = WasmEngine::new(&config);
        let bounded = bound_wasm_memories(wasm)?;
        let terminal = Module::new(&engine, bounded.as_slice()).context("compile Ghostty VT")?;
        let callbacks =
            Module::new(&engine, CALLBACKS).context("compile VT callback trampoline")?;
        Ok(Self {
            engine,
            terminal,
            callbacks,
        })
    }

    pub fn terminal(&self, cols: u32, rows: u32) -> Result<Terminal> {
        Terminal::instantiate(self, cols, rows)
    }
}

#[derive(Default)]
struct Host {
    memory: Option<Memory>,
    terminal: u32,
    cols: u32,
    rows: u32,
    cell_width: u32,
    cell_height: u32,
    replies: Vec<u8>,
    callback_error: Option<String>,
    dirty_title: bool,
    dirty_pwd: bool,
    title: String,
    pwd: String,
    revision: u64,
    records: BTreeMap<String, Status>,
    prompts: Vec<Prompt>,
    abi: Manifest,
    exports: BTreeMap<String, Func>,
    scratch: u32,
}

pub struct Terminal {
    store: Store<Host>,
    scratch: u32,
}

#[derive(Clone, Deserialize, Default)]
struct AbiInfo {
    #[serde(alias = "PointerSize", default)]
    pointer_size: u32,
    #[serde(alias = "UsizeSize", default)]
    usize_size: u32,
    #[serde(rename = "Endian", alias = "endian")]
    endian: String,
}

#[derive(Clone, Deserialize, Default)]
struct AbiField {
    #[serde(rename = "Offset", alias = "offset")]
    offset: u32,
    #[serde(rename = "Size", alias = "size")]
    size: u32,
}

#[derive(Clone, Deserialize, Default)]
struct AbiType {
    #[serde(rename = "Size", alias = "size", default)]
    size: u32,
    #[serde(rename = "Fields", alias = "fields", default)]
    fields: BTreeMap<String, AbiField>,
    #[serde(rename = "Values", alias = "values", default)]
    values: BTreeMap<String, i64>,
}

#[derive(Clone, Deserialize, Default)]
struct Manifest {
    #[serde(rename = "ABI", alias = "abi", default)]
    abi: AbiInfo,
    #[serde(rename = "Types", alias = "types", default)]
    types: BTreeMap<String, AbiType>,
}

impl Manifest {
    fn value(&self, ty: &str, key: &str) -> Result<u32> {
        Ok(u32::try_from(
            *self
                .types
                .get(ty)
                .and_then(|t| t.values.get(key))
                .ok_or_else(|| anyhow!("missing VT ABI selector {ty}.{key}"))?,
        )?)
    }
    fn field(&self, ty: &str, key: &str) -> Result<AbiField> {
        let t = self
            .types
            .get(ty)
            .ok_or_else(|| anyhow!("missing VT ABI type {ty}"))?;
        let f = t
            .fields
            .get(key)
            .ok_or_else(|| anyhow!("missing VT ABI field {ty}.{key}"))?;
        ensure!(
            f.size != 0 && u64::from(f.offset) + u64::from(f.size) <= u64::from(t.size),
            "invalid VT ABI field {ty}.{key}"
        );
        Ok(f.clone())
    }
}

#[derive(Clone, serde::Serialize)]
struct Status {
    id: String,
    app: String,
    title: String,
    message: String,
    state: String,
    kind: String,
    progress: i32,
}

#[derive(Clone, serde::Serialize)]
struct Prompt {
    line: u32,
    kind: String,
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
}

impl Terminal {
    fn instantiate(engine: &Engine, cols: u32, rows: u32) -> Result<Self> {
        ensure!(
            cols > 0 && rows > 0 && cols <= u16::MAX as u32 && rows <= u16::MAX as u32,
            "invalid terminal dimensions"
        );
        let mut store = Store::new(
            &engine.engine,
            Host {
                cols,
                rows,
                ..Host::default()
            },
        );
        store
            .set_fuel(CALL_FUEL)
            .context("configure initial VT fuel")?;
        let mut linker = Linker::new(&engine.engine);
        linker.func_wrap("env", "log", |_c: Caller<'_, Host>, _a: i32, _b: i32| {})?;
        install_host(&mut linker)?;

        let instance = linker
            .instantiate_and_start(&mut store, &engine.terminal)
            .context("instantiate and start Ghostty VT")?;
        cache_exports(
            &instance,
            &mut store,
            &[
                "ghostty_type_json",
                "ghostty_wasm_alloc",
                "ghostty_wasm_free",
                "ghostty_wasm_alloc_opaque",
                "ghostty_wasm_free_opaque",
                "ghostty_terminal_new",
                "ghostty_terminal_free",
                "ghostty_terminal_set",
                "ghostty_terminal_get",
                "ghostty_terminal_vt_write",
                "ghostty_terminal_resize",
                "ghostty_alloc",
                "ghostty_free",
                "ghostty_sys_set",
                "ghostty_formatter_terminal_new",
                "ghostty_formatter_set",
                "ghostty_formatter_format_buf",
                "ghostty_formatter_free",
            ],
        )?;
        let memory = instance
            .get_export(&store, "memory")
            .and_then(Extern::into_memory)
            .ok_or_else(|| anyhow!("missing Ghostty memory"))?;
        store.data_mut().memory = Some(memory);
        check_memory(&store)?;

        let pointer = call_u0(&mut store, "ghostty_type_json")? as u32;
        let raw = c_string(&store, pointer, MAX_CAPTURE)?;
        store.data_mut().abi =
            serde_json::from_slice(&raw).context("decode Ghostty ABI manifest")?;
        let abi = store.data().abi.clone();
        ensure!(
            abi.abi.pointer_size == 4 && abi.abi.usize_size == 4 && abi.abi.endian == "little",
            "unsupported Ghostty VT ABI: pointer_size={}, usize_size={}, endian={:?}",
            abi.abi.pointer_size,
            abi.abi.usize_size,
            abi.abi.endian
        );
        let scratch = alloc(&mut store, 8)?;
        write32(&mut store, scratch, 0)?;
        result_call(
            &mut store,
            "ghostty_terminal_new",
            &[0, scratch as i64, cols as i64, rows as i64],
        )?;
        let terminal = read32(&store, scratch)?;
        ensure!(terminal != 0, "Ghostty did not create a terminal");
        store.data_mut().terminal = terminal;
        store.data_mut().scratch = scratch;

        for (name, value) in [
            ("SCROLLBACK_MAX_BYTES", 16_u32 << 20),
            ("SCROLLBACK_MAX_LINES", 2000),
            ("KITTY_IMAGE_STORAGE_LIMIT", 16 << 20),
        ] {
            let key = abi.value("GhosttyTerminalOption", name)?;
            write64(&mut store, scratch, value)?;
            result_call(
                &mut store,
                "ghostty_terminal_set",
                &[terminal as i64, key as i64, scratch as i64],
            )?;
        }

        let table = instance
            .get_export(&store, "__indirect_function_table")
            .and_then(Extern::into_table)
            .ok_or_else(|| anyhow!("missing Ghostty callback table"))?;
        linker.define("vt", "memory", Extern::Memory(memory))?;
        linker.define("vt", "__indirect_function_table", Extern::Table(table))?;
        let shim = linker
            .instantiate_and_start(&mut store, &engine.callbacks)
            .context("instantiate and start VT callbacks")?;
        for (install, option) in [
            ("install_write", "WRITE_PTY"),
            ("install_reset", "RESET"),
            ("install_status", "PROGRAM_STATUS"),
            ("install_prompt", "SEMANTIC_PROMPT"),
            ("install_title", "TITLE_CHANGED"),
            ("install_pwd", "PWD_CHANGED"),
            ("install_size", "SIZE"),
        ] {
            let index = export_call0(&shim, &mut store, install)?;
            ensure!(index != u32::MAX, "failed to install VT callback {install}");
            let key = abi.value("GhosttyTerminalOption", option)?;
            result_call(
                &mut store,
                "ghostty_terminal_set",
                &[terminal as i64, key as i64, index as i64],
            )?;
        }
        let decoder = export_call0(&shim, &mut store, "install_png")?;
        ensure!(decoder != u32::MAX, "failed to install server PNG decoder");
        let key = abi.value("GhosttySysOption", "DECODE_PNG")?;
        result_call(&mut store, "ghostty_sys_set", &[key as i64, decoder as i64])?;
        Ok(Self { store, scratch })
    }

    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<u8>> {
        self.begin_call()?;
        if data.is_empty() {
            return Ok(Vec::new());
        }
        ensure!(data.len() <= MAX_FEED, "VT feed exceeds 64 KiB");
        let ptr = alloc(&mut self.store, data.len() as u32)?;
        self.store
            .data()
            .memory
            .ok_or_else(|| anyhow!("missing Ghostty memory"))?
            .write(&mut self.store, ptr as usize, data)
            .context("VT feed memory overflow")?;
        self.store.data_mut().replies.clear();
        self.store.data_mut().callback_error = None;
        let terminal = self.store.data().terminal;
        let outcome = call_untyped(
            &mut self.store,
            "ghostty_terminal_vt_write",
            &[
                Val::I32(terminal as i32),
                Val::I32(ptr as i32),
                Val::I32(data.len() as i32),
            ],
        );
        let _ = call_untyped(
            &mut self.store,
            "ghostty_wasm_free",
            &[Val::I32(ptr as i32), Val::I32(data.len() as i32)],
        );
        outcome?;
        self.refresh_title_pwd()?;
        if let Some(error) = self.store.data().callback_error.clone() {
            bail!("{error}");
        }
        check_memory(&self.store)?;
        Ok(std::mem::take(&mut self.store.data_mut().replies))
    }

    pub fn resize(
        &mut self,
        cols: u32,
        rows: u32,
        cell_width: u32,
        cell_height: u32,
    ) -> Result<()> {
        ensure!(
            cols > 0 && rows > 0 && cols <= u16::MAX as u32 && rows <= u16::MAX as u32,
            "invalid terminal dimensions"
        );
        self.begin_call()?;
        let terminal = self.store.data().terminal;
        result_call(
            &mut self.store,
            "ghostty_terminal_resize",
            &[
                terminal.into(),
                cols.into(),
                rows.into(),
                cell_width.into(),
                cell_height.into(),
            ],
        )?;
        let host = self.store.data_mut();
        host.cols = cols;
        host.rows = rows;
        host.cell_width = cell_width;
        host.cell_height = cell_height;
        Ok(())
    }

    pub fn capture(&mut self, format: &str, trim: bool, unwrap: bool) -> Result<Vec<u8>> {
        self.begin_call()?;
        let (kind, make_html) = match format {
            "text" => ("PLAIN", false),
            "vt" => ("VT", false),
            "html" => ("HTML", true),
            _ => bail!("unsupported capture format"),
        };
        let abi = self.store.data().abi.clone();
        let emit = abi.value("GhosttyFormatterFormat", kind)?;
        let terminal = self.store.data().terminal;
        result_call(
            &mut self.store,
            "ghostty_formatter_terminal_new",
            &[0, (self.scratch as i64), terminal.into()],
        )?;
        let formatter = read32(&self.store, self.scratch)?;
        for (name, value) in [
            ("EMIT", emit),
            ("TRIM", u32::from(trim)),
            ("UNWRAP", u32::from(unwrap)),
        ] {
            let option = abi.value("GhosttyFormatterOption", name)?;
            write32(&mut self.store, self.scratch, value)?;
            if let Err(error) = result_call(
                &mut self.store,
                "ghostty_formatter_set",
                &[formatter.into(), option.into(), (self.scratch as i64)],
            ) {
                let _ = call_untyped(
                    &mut self.store,
                    "ghostty_formatter_free",
                    &[Val::I32(formatter as i32)],
                );
                return Err(error);
            }
        }
        if format == "vt" {
            let key = abi.value("GhosttyFormatterOption", "EXTRAS")?;
            write32(&mut self.store, self.scratch, 4095)?;
            result_call(
                &mut self.store,
                "ghostty_formatter_set",
                &[formatter.into(), key.into(), (self.scratch as i64)],
            )?;
        }
        let measured = call_untyped(
            &mut self.store,
            "ghostty_formatter_format_buf",
            &[
                Val::I32(formatter as i32),
                Val::I32(0),
                Val::I32(0),
                Val::I32(self.scratch as i32),
            ],
        );
        if let Err(error) = measured {
            self.free_formatter(formatter);
            return Err(error);
        }
        let status = match measured {
            Ok(value) => value,
            Err(error) => {
                self.free_formatter(formatter);
                return Err(error);
            }
        };
        let result = val_i32(&status).unwrap_or(0);
        if result != 0 && result != -3 {
            self.free_formatter(formatter);
            bail!("measure capture: Ghostty status {result}");
        }
        let required = read32(&self.store, self.scratch).unwrap_or(MAX_CAPTURE + 1);
        if required == 0 {
            self.free_formatter(formatter);
            return Ok(Vec::new());
        }
        ensure!(required <= MAX_CAPTURE, "capture exceeds 4 MiB");
        let ptr = match alloc(&mut self.store, required) {
            Ok(v) => v,
            Err(e) => {
                self.free_formatter(formatter);
                return Err(e);
            }
        };
        let formatted = result_call(
            &mut self.store,
            "ghostty_formatter_format_buf",
            &[
                formatter.into(),
                ptr.into(),
                required.into(),
                (self.scratch as i64),
            ],
        );
        self.free_formatter(formatter);
        if let Err(error) = formatted {
            let _ = call_untyped(
                &mut self.store,
                "ghostty_wasm_free",
                &[Val::I32(ptr as i32), Val::I32(required as i32)],
            );
            return Err(error);
        }
        let length = read32(&self.store, self.scratch)?;
        ensure!(length <= required, "invalid capture length");
        let mut out = vec![0; length as usize];
        self.store
            .data()
            .memory
            .unwrap()
            .read(&self.store, ptr as usize, &mut out)
            .context("invalid capture memory")?;
        let _ = call_untyped(
            &mut self.store,
            "ghostty_wasm_free",
            &[Val::I32(ptr as i32), Val::I32(required as i32)],
        );
        if make_html {
            sanitize_html(&out)
        } else {
            Ok(out)
        }
    }

    pub fn info(&self) -> serde_json::Value {
        let h = self.store.data();
        let records: Vec<_> = h.records.values().cloned().collect();
        let selected = records
            .iter()
            .max_by_key(|record| match record.state.as_str() {
                "error" => 5,
                "blocked" => 4,
                "working" => 3,
                "done" => 2,
                "idle" => 1,
                _ => 0,
            });
        let mut summary = serde_json::json!({
            "title": h.title,
            "pwd": h.pwd,
            "revision": h.revision,
            "records": records,
            "prompts": h.prompts,
            "state": selected.map_or("", |record| record.state.as_str()),
        });
        if let Some(record) = selected {
            summary["app"] = serde_json::json!(record.app);
            summary["message"] = serde_json::json!(record.message);
            summary["status_title"] = serde_json::json!(record.title);
            if record.progress >= 0 {
                summary["progress"] = serde_json::json!(record.progress);
            }
        }
        summary["program_status"] = serde_json::json!({ "records": summary["records"] });
        summary
    }

    fn begin_call(&mut self) -> Result<()> {
        self.store.set_fuel(CALL_FUEL).context("reset VT fuel")?;
        check_memory(&self.store)
    }
    fn free_formatter(&mut self, formatter: u32) {
        let _ = call_untyped(
            &mut self.store,
            "ghostty_formatter_free",
            &[Val::I32(formatter as i32)],
        );
    }
    fn refresh_title_pwd(&mut self) -> Result<()> {
        let (title, pwd) = {
            let h = self.store.data();
            (h.dirty_title, h.dirty_pwd)
        };
        if title {
            let value = pull_string(&mut self.store, self.scratch, "TITLE")?;
            let h = self.store.data_mut();
            h.title = value;
            h.dirty_title = false;
            h.revision += 1;
        }
        if pwd {
            let value = pull_string(&mut self.store, self.scratch, "PWD")?;
            let h = self.store.data_mut();
            h.pwd = value;
            h.dirty_pwd = false;
            h.revision += 1;
        }
        Ok(())
    }
}

fn sanitize_html(source: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        source.len() <= MAX_CAPTURE as usize,
        "capture exceeds 4 MiB"
    );
    let mut out = Vec::with_capacity(source.len().min(256 << 10));
    let mut open: Vec<String> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();
    let mut head_added = false;
    let mut pos = 0;
    while pos < source.len() {
        let next = source[pos..]
            .iter()
            .position(|byte| *byte == b'<')
            .map(|offset| pos + offset)
            .unwrap_or(source.len());
        if blocked.is_empty() && next > pos {
            append_html(&mut out, &source[pos..next])?;
        }
        if next == source.len() {
            break;
        }
        if starts_ascii_case(&source[next..], b"<!--") {
            pos = source[next + 4..]
                .iter()
                .position(|byte| *byte == b'-')
                .and_then(|offset| {
                    let candidate = next + 4 + offset;
                    source[candidate..]
                        .starts_with(b"-->")
                        .then_some(candidate + 3)
                })
                .unwrap_or(source.len());
            continue;
        }
        if starts_ascii_case(&source[next..], b"<!doctype") {
            let Some(end) = tag_end(source, next + 1)? else {
                if blocked.is_empty() {
                    append_html(&mut out, b"&lt;")?;
                }
                pos = next + 1;
                continue;
            };
            if blocked.is_empty() {
                append_html(&mut out, b"<!DOCTYPE html>")?;
            }
            pos = end + 1;
            continue;
        }
        let Some((end, token)) = parse_html_tag(source, next)? else {
            if blocked.is_empty() {
                append_html(&mut out, b"&lt;")?;
            }
            pos = next + 1;
            continue;
        };
        pos = end + 1;
        let name = token.name;
        if !blocked.is_empty() {
            if name == blocked[blocked.len() - 1] {
                if token.closing {
                    blocked.pop();
                } else if !token.void {
                    blocked.push(name);
                }
            }
            continue;
        }
        if dangerous_block(&name) {
            if !token.closing && !token.void {
                blocked.push(name);
            }
            continue;
        }
        if drop_element(&name) {
            continue;
        }
        if !allowed_element(&name) {
            continue;
        }
        if token.closing {
            if name != "html" && name != "head" && name != "body" || open.contains(&name) {
                let Some(index) = open.iter().rposition(|item| item == &name) else {
                    continue;
                };
                for item in open[index..].iter().rev() {
                    append_html(&mut out, b"</")?;
                    append_html(&mut out, item.as_bytes())?;
                    append_html(&mut out, b">")?;
                }
                open.truncate(index);
            }
            continue;
        }
        append_html(&mut out, b"<")?;
        append_html(&mut out, name.as_bytes())?;
        for (attribute, value) in token.attributes {
            if attribute == "style" {
                let safe = sanitize_style(&value);
                if !safe.is_empty() {
                    append_attribute(&mut out, "style", &safe)?;
                }
            } else if attribute == "href" && name == "a" && safe_href(&value) {
                append_attribute(&mut out, "href", &value)?;
            } else if attribute == "charset"
                && name == "meta"
                && value.eq_ignore_ascii_case("utf-8")
            {
                append_attribute(&mut out, "charset", "utf-8")?;
            }
        }
        append_html(&mut out, b">")?;
        if name == "head" {
            append_csp_meta(&mut out)?;
            head_added = true;
        }
        if !token.void && !void_element(&name) {
            open.push(name);
        }
    }
    for name in open.iter().rev() {
        append_html(&mut out, b"</")?;
        append_html(&mut out, name.as_bytes())?;
        append_html(&mut out, b">")?;
    }
    if !head_added {
        let mut document = Vec::new();
        append_html(
            &mut document,
            b"<!DOCTYPE html><html><head><meta charset=\"utf-8\">",
        )?;
        append_csp_meta(&mut document)?;
        append_html(&mut document, b"</head><body>")?;
        append_html(&mut document, &out)?;
        append_html(&mut document, b"</body></html>")?;
        out = document;
    }
    Ok(out)
}

struct HtmlTag {
    name: String,
    closing: bool,
    void: bool,
    attributes: Vec<(String, String)>,
}

fn parse_html_tag(source: &[u8], start: usize) -> Result<Option<(usize, HtmlTag)>> {
    let Some(end) = tag_end(source, start + 1)? else {
        return Ok(None);
    };
    let mut pos = start + 1;
    let closing = source.get(pos) == Some(&b'/');
    if closing {
        pos += 1;
    }
    while source.get(pos).is_some_and(u8::is_ascii_whitespace) {
        pos += 1;
    }
    let name_start = pos;
    while source
        .get(pos)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
    {
        pos += 1;
    }
    if pos == name_start {
        return Ok(None);
    }
    let name = String::from_utf8_lossy(&source[name_start..pos]).to_ascii_lowercase();
    let mut attributes = Vec::new();
    if !closing {
        while pos < end {
            while source.get(pos).is_some_and(u8::is_ascii_whitespace) {
                pos += 1;
            }
            if pos >= end || source[pos] == b'/' {
                break;
            }
            let key_start = pos;
            while source.get(pos).is_some_and(|byte| {
                byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_' | b':' | b'.')
            }) {
                pos += 1;
            }
            if pos == key_start {
                pos += 1;
                continue;
            }
            let key = String::from_utf8_lossy(&source[key_start..pos]).to_ascii_lowercase();
            while source.get(pos).is_some_and(u8::is_ascii_whitespace) {
                pos += 1;
            }
            let mut value = String::new();
            if source.get(pos) == Some(&b'=') {
                pos += 1;
                while source.get(pos).is_some_and(u8::is_ascii_whitespace) {
                    pos += 1;
                }
                if matches!(source.get(pos), Some(b'\'' | b'"')) {
                    let quote = source[pos];
                    pos += 1;
                    let value_start = pos;
                    while pos < end && source[pos] != quote {
                        pos += 1;
                    }
                    value = String::from_utf8_lossy(&source[value_start..pos]).into_owned();
                    if pos < end {
                        pos += 1;
                    }
                } else {
                    let value_start = pos;
                    while pos < end
                        && !source[pos].is_ascii_whitespace()
                        && !matches!(source[pos], b'>' | b'/')
                    {
                        pos += 1;
                    }
                    value = String::from_utf8_lossy(&source[value_start..pos]).into_owned();
                }
            }
            attributes.push((key, value));
        }
    }
    let void = source[start + 1..end]
        .iter()
        .rev()
        .take_while(|byte| byte.is_ascii_whitespace())
        .next()
        .is_none()
        && source.get(end.saturating_sub(1)) == Some(&b'/');
    Ok(Some((
        end,
        HtmlTag {
            name,
            closing,
            void,
            attributes,
        },
    )))
}

fn tag_end(source: &[u8], start: usize) -> Result<Option<usize>> {
    let mut quote = None;
    for (offset, byte) in source[start..].iter().enumerate() {
        if let Some(expected) = quote {
            if *byte == expected {
                quote = None;
            }
        } else if matches!(*byte, b'\'' | b'"') {
            quote = Some(*byte);
        } else if *byte == b'>' {
            return Ok(Some(start + offset));
        }
    }
    Ok(None)
}

fn append_html(out: &mut Vec<u8>, bytes: &[u8]) -> Result<()> {
    ensure!(
        out.len() + bytes.len() <= MAX_CAPTURE as usize,
        "sanitised HTML exceeds 4 MiB"
    );
    out.extend_from_slice(bytes);
    Ok(())
}

fn append_attribute(out: &mut Vec<u8>, name: &str, value: &str) -> Result<()> {
    append_html(out, b" ")?;
    append_html(out, name.as_bytes())?;
    append_html(out, b"=\"")?;
    for &byte in value.as_bytes() {
        match byte {
            b'&' => append_html(out, b"&amp;")?,
            b'"' => append_html(out, b"&quot;")?,
            b'<' => append_html(out, b"&lt;")?,
            b'>' => append_html(out, b"&gt;")?,
            _ => append_html(out, &[byte])?,
        }
    }
    append_html(out, b"\"")?;
    Ok(())
}

fn append_csp_meta(out: &mut Vec<u8>) -> Result<()> {
    append_html(
        out,
        b"<meta http-equiv=\"Content-Security-Policy\" content=\"",
    )?;
    append_html(out, EXPORT_CSP.as_bytes())?;
    append_html(out, b"\">")
}

fn allowed_element(name: &str) -> bool {
    matches!(
        name,
        "html" | "head" | "body" | "meta" | "title" | "pre" | "code" | "div" | "span" | "a" | "br"
    )
}

fn dangerous_block(name: &str) -> bool {
    matches!(
        name,
        "script"
            | "style"
            | "iframe"
            | "object"
            | "form"
            | "svg"
            | "math"
            | "template"
            | "noscript"
    )
}

fn drop_element(name: &str) -> bool {
    matches!(
        name,
        "base" | "embed" | "link" | "img" | "audio" | "video" | "source" | "input" | "button"
    )
}

fn void_element(name: &str) -> bool {
    matches!(
        name,
        "base" | "br" | "embed" | "img" | "meta" | "link" | "source"
    )
}

fn sanitize_style(source: &str) -> String {
    let mut safe = Vec::new();
    for declaration in source.split(';') {
        let Some((property, value)) = declaration.split_once(':') else {
            continue;
        };
        let property = property.trim().to_ascii_lowercase();
        let value = value.trim();
        if safe_style_value(&property, value) {
            safe.push(format!("{property}: {value}"));
        }
    }
    safe.join("; ")
}

fn safe_style_value(property: &str, value: &str) -> bool {
    if value.is_empty()
        || value
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\\' | '<' | '>' | '\'' | '"' | '&' | '@'))
    {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    if lower.contains("url(") || lower.contains("expression(") || lower.contains("var(") {
        return false;
    }
    match property {
        "font-family" => value.eq_ignore_ascii_case("monospace"),
        "white-space" => matches!(lower.as_str(), "pre" | "pre-wrap"),
        "display" => matches!(lower.as_str(), "inline" | "inline-block" | "block"),
        "font-weight" => {
            matches!(lower.as_str(), "normal" | "bold")
                || matches!(
                    lower.as_str(),
                    "100" | "200" | "300" | "400" | "500" | "600" | "700" | "800" | "900"
                )
        }
        "font-style" => matches!(lower.as_str(), "normal" | "italic"),
        "visibility" => matches!(lower.as_str(), "visible" | "hidden"),
        "opacity" => value
            .parse::<f32>()
            .is_ok_and(|number| (0.0..=1.0).contains(&number)),
        "filter" => lower == "invert(100%)",
        "text-decoration-style" => matches!(
            lower.as_str(),
            "solid" | "double" | "dotted" | "dashed" | "wavy"
        ),
        "text-decoration-line" => lower
            .split_ascii_whitespace()
            .all(|part| matches!(part, "none" | "underline" | "line-through" | "overline")),
        "color" | "background-color" | "text-decoration-color" => safe_css_color(&lower),
        _ => false,
    }
}

fn safe_css_color(value: &str) -> bool {
    if let Some(hex) = value.strip_prefix('#') {
        return matches!(hex.len(), 3 | 4 | 6 | 8)
            && hex.bytes().all(|byte| byte.is_ascii_hexdigit());
    }
    if ["rgb(", "rgba(", "hsl(", "hsla("]
        .iter()
        .any(|prefix| value.starts_with(prefix))
        && value.ends_with(')')
    {
        return value[value.find('(').unwrap() + 1..value.len() - 1]
            .bytes()
            .all(|byte| {
                byte.is_ascii_digit()
                    || byte.is_ascii_whitespace()
                    || matches!(byte, b',' | b'.' | b'%' | b'+' | b'-')
            });
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphabetic() || byte == b'-')
}

fn safe_href(value: &str) -> bool {
    if value.is_empty()
        || value.trim() != value
        || value
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '\\' | '&' | '<' | '>' | '"' | '\''))
    {
        return false;
    }
    let end = value.find([':', '/', '?', '#']).unwrap_or(value.len());
    if end < value.len() && value.as_bytes()[end] == b':' {
        let scheme = &value[..end];
        return !scheme.is_empty()
            && scheme
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
            && matches!(
                scheme.to_ascii_lowercase().as_str(),
                "http" | "https" | "mailto" | "ftp" | "ssh" | "gemini" | "gopher" | "tel"
            );
    }
    true
}

fn starts_ascii_case(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix)
}

fn install_host(linker: &mut Linker<Host>) -> Result<()> {
    // Ghostty only observes these callback numbers; WASM is never given a host capability.
    linker.func_wrap("rexweb_host", "reset", host_reset)?;
    linker.func_wrap(
        "rexweb_host",
        "title_changed",
        |mut c: Caller<'_, Host>, _: i32, _: i32| {
            c.data_mut().dirty_title = true;
        },
    )?;
    linker.func_wrap(
        "rexweb_host",
        "pwd_changed",
        |mut c: Caller<'_, Host>, _: i32, _: i32| {
            c.data_mut().dirty_pwd = true;
        },
    )?;
    linker.func_wrap("rexweb_host", "write_pty", host_write)?;
    linker.func_wrap("rexweb_host", "size", host_size)?;
    linker.func_wrap("rexweb_host", "program_status", host_status)?;
    linker.func_wrap("rexweb_host", "semantic_prompt", host_prompt)?;
    linker.func_wrap("rexweb_host", "decode_png", host_png)?;
    Ok(())
}

fn host_write(mut c: Caller<'_, Host>, _a: i32, _b: i32, ptr: i32, len: i32) {
    let len = match u32::try_from(len) {
        Ok(v) => v,
        Err(_) => return fail(&mut c, "invalid Ghostty response length"),
    };
    if len as u64 + c.data().replies.len() as u64 > 64 << 10 {
        return fail(&mut c, "Ghostty response limit exceeded");
    }
    let Some(memory) = c.data().memory else {
        return fail(&mut c, "missing Ghostty memory");
    };
    let mut bytes = vec![0; len as usize];
    if memory.read(&c, ptr as u32 as usize, &mut bytes).is_err() {
        return fail(&mut c, "invalid Ghostty response");
    }
    c.data_mut().replies.extend_from_slice(&bytes);
}

fn host_size(mut c: Caller<'_, Host>, _a: i32, _b: i32, ptr: i32) -> i32 {
    let (abi, rows, cols, cw, ch) = {
        let h = c.data();
        (h.abi.clone(), h.rows, h.cols, h.cell_width, h.cell_height)
    };
    let base = ptr as u32;
    let r = (|| -> Result<()> {
        put_u16_memory(
            &mut c,
            base + abi.field("GhosttySizeReportSize", "rows")?.offset,
            rows as u16,
        )?;
        put_u16_memory(
            &mut c,
            base + abi.field("GhosttySizeReportSize", "columns")?.offset,
            cols as u16,
        )?;
        put_u32_memory(
            &mut c,
            base + abi.field("GhosttySizeReportSize", "cell_width")?.offset,
            cw,
        )?;
        put_u32_memory(
            &mut c,
            base + abi.field("GhosttySizeReportSize", "cell_height")?.offset,
            ch,
        )?;
        Ok(())
    })();
    if let Err(e) = r {
        fail(&mut c, &e.to_string());
        0
    } else {
        1
    }
}

fn host_reset(mut c: Caller<'_, Host>, _a: i32, _b: i32) {
    let h = c.data_mut();
    h.records.clear();
    h.prompts.clear();
    h.title.clear();
    h.dirty_title = true;
    h.dirty_pwd = true;
    h.revision += 1;
}

fn host_status(mut c: Caller<'_, Host>, _a: i32, _b: i32, ptr: i32) {
    if let Err(e) = (|| -> Result<()> {
        let base = ptr as u32;
        let abi = c.data().abi.clone();
        let memory = c.data().memory.context("missing memory")?;
        let state = memory_u32(
            &c,
            memory,
            base + abi.field("GhosttyTerminalProgramStatus", "state")?.offset,
        )?;
        let kind = memory_u32(
            &c,
            memory,
            base + abi.field("GhosttyTerminalProgramStatus", "kind")?.offset,
        )?;
        let progress = memory_u8(
            &c,
            memory,
            base + abi
                .field("GhosttyTerminalProgramStatus", "progress")?
                .offset,
        )? as i8;
        let record = Status {
            state: enum_name(&abi, "GhosttyProgramStatusState", state),
            kind: enum_name(&abi, "GhosttyProgramStatusKind", kind),
            progress: progress.into(),
            id: abi_string(
                &c,
                memory,
                &abi,
                base,
                "GhosttyTerminalProgramStatus",
                "id",
                128,
            )?,
            app: abi_string(
                &c,
                memory,
                &abi,
                base,
                "GhosttyTerminalProgramStatus",
                "app",
                128,
            )?,
            title: abi_string(
                &c,
                memory,
                &abi,
                base,
                "GhosttyTerminalProgramStatus",
                "title",
                256,
            )?,
            message: abi_string(
                &c,
                memory,
                &abi,
                base,
                "GhosttyTerminalProgramStatus",
                "message",
                1024,
            )?,
        };
        let h = c.data_mut();
        if record.state == "clear" {
            h.records.remove(&record.id);
        } else if h.records.contains_key(&record.id) || h.records.len() < 32 {
            h.records.insert(record.id.clone(), record);
        }
        h.revision += 1;
        Ok(())
    })() {
        fail(&mut c, &e.to_string());
    }
}

fn host_prompt(mut c: Caller<'_, Host>, _a: i32, _b: i32, ptr: i32) {
    if let Err(e) = (|| -> Result<()> {
        let base = ptr as u32;
        let abi = c.data().abi.clone();
        let memory = c.data().memory.context("missing memory")?;
        let kind = memory_u32(
            &c,
            memory,
            base + abi.field("GhosttyTerminalSemanticPrompt", "kind")?.offset,
        )?;
        let command = abi_string(
            &c,
            memory,
            &abi,
            base,
            "GhosttyTerminalSemanticPrompt",
            "command",
            512,
        )?;
        let terminal = c.data().terminal;
        let key_total = abi.value("GhosttyTerminalData", "TOTAL_ROWS")?;
        let key_y = abi.value("GhosttyTerminalData", "CURSOR_Y")?;
        let scratch = c.data().scratch;
        ensure!(scratch != 0, "unavailable VT callback scratch");
        let total = callback_get32(&mut c, terminal, key_total, scratch)?;
        let row = callback_get32(&mut c, terminal, key_y, scratch)?;
        let has = memory_u8(
            &c,
            memory,
            base + abi
                .field("GhosttyTerminalSemanticPrompt", "has_exit_code")?
                .offset,
        )?;
        let code = if has != 0 {
            Some(memory_u32(
                &c,
                memory,
                base + abi
                    .field("GhosttyTerminalSemanticPrompt", "exit_code")?
                    .offset,
            )? as i32)
        } else {
            None
        };
        let scrollback = total.saturating_sub(c.data().rows);
        let h = c.data_mut();
        h.prompts.push(Prompt {
            line: scrollback + row,
            kind: enum_name(&abi, "GhosttySemanticPromptKind", kind),
            command,
            exit_code: code,
        });
        if h.prompts.len() > 256 {
            let drop = h.prompts.len() - 256;
            h.prompts.drain(..drop);
        }
        h.revision += 1;
        Ok(())
    })() {
        fail(&mut c, &e.to_string());
    }
}

fn host_png(mut c: Caller<'_, Host>, _a: i32, allocator: i32, ptr: i32, len: i32, out: i32) -> i32 {
    const LIMIT: usize = 8 << 20;
    let Ok(len) = usize::try_from(len) else {
        return 0;
    };
    if len == 0 || len > LIMIT {
        return 0;
    }
    if let Err(e) = (|| -> Result<()> {
        let memory = c.data().memory.context("missing memory")?;
        let mut encoded = vec![0; len];
        memory.read(&c, ptr as u32 as usize, &mut encoded)?;
        decode_and_install_png(&mut c, memory, allocator as u32, out as u32, &encoded)
    })() {
        let _ = e;
        return 0;
    }
    1
}

fn decode_and_install_png<C: AsContextMut<Data = Host>>(
    c: &mut C,
    memory: Memory,
    allocator: u32,
    out: u32,
    encoded: &[u8],
) -> Result<()> {
    use std::io::Cursor;
    let decoder = png::Decoder::new(Cursor::new(encoded));
    let mut reader = decoder.read_info().context("decode PNG header")?;
    let (width, height) = (reader.info().width, reader.info().height);
    ensure!(
        width > 0
            && height > 0
            && width <= 4096
            && height <= 4096
            && u64::from(width) * u64::from(height) <= 4 * 1024 * 1024,
        "PNG dimensions exceed limit"
    );
    let buffer_size = reader
        .output_buffer_size()
        .context("PNG output size unavailable")?;
    ensure!(buffer_size <= 16 << 20, "PNG output exceeds limit");
    let mut bytes = vec![0; buffer_size];
    let output = reader.next_frame(&mut bytes).context("decode PNG")?;
    ensure!(output.buffer_size() <= 16 << 20, "PNG output exceeds limit");
    let decoded = &bytes[..output.buffer_size()];
    let rgba = match output.color_type {
        png::ColorType::Rgba => decoded.to_vec(),
        png::ColorType::Rgb => {
            let src = decoded;
            let mut dst = Vec::with_capacity(src.len() / 3 * 4);
            for p in src.chunks_exact(3) {
                dst.extend_from_slice(p);
                dst.push(255);
            }
            dst
        }
        png::ColorType::Grayscale => {
            let src = decoded;
            let mut dst = Vec::with_capacity(src.len() * 4);
            for &v in src {
                dst.extend_from_slice(&[v, v, v, 255]);
            }
            dst
        }
        png::ColorType::GrayscaleAlpha => {
            let src = decoded;
            let mut dst = Vec::with_capacity(src.len() / 2 * 4);
            for p in src.chunks_exact(2) {
                dst.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
            dst
        }
        _ => bail!("unsupported PNG color type"),
    };
    let pointer = val_i32(&call_dynamic(
        c,
        "ghostty_alloc",
        &[Val::I32(allocator as i32), Val::I32(rgba.len() as i32)],
    )?)
    .unwrap_or(0) as u32;
    ensure!(pointer != 0, "Ghostty image allocation failed");
    if memory.write(&mut *c, pointer as usize, &rgba).is_err() {
        let _ = call_dynamic(
            c,
            "ghostty_free",
            &[
                Val::I32(allocator as i32),
                Val::I32(pointer as i32),
                Val::I32(rgba.len() as i32),
            ],
        );
        bail!("write decoded PNG");
    }
    let abi = c.as_context().data().abi.clone();
    put32_dynamic(
        c,
        out + abi.field("GhosttySysImage", "width")?.offset,
        width,
    )?;
    put32_dynamic(
        c,
        out + abi.field("GhosttySysImage", "height")?.offset,
        height,
    )?;
    put32_dynamic(
        c,
        out + abi.field("GhosttySysImage", "data")?.offset,
        pointer,
    )?;
    put32_dynamic(
        c,
        out + abi.field("GhosttySysImage", "data_len")?.offset,
        rgba.len() as u32,
    )?;
    Ok(())
}

fn callback_get32<C: AsContextMut<Data = Host>>(
    c: &mut C,
    terminal: u32,
    key: u32,
    scratch: u32,
) -> Result<u32> {
    result_dynamic(
        c,
        "ghostty_terminal_get",
        &[
            Val::I32(terminal as i32),
            Val::I32(key as i32),
            Val::I32(scratch as i32),
        ],
    )?;
    dynamic_read32(c, scratch)
}

fn fail(c: &mut Caller<'_, Host>, message: &str) {
    if c.data().callback_error.is_none() {
        c.data_mut().callback_error = Some(message.to_owned());
    }
}
fn enum_name(abi: &Manifest, ty: &str, value: u32) -> String {
    abi.types
        .get(ty)
        .and_then(|t| {
            t.values
                .iter()
                .find(|(k, v)| *k != "MAX_VALUE" && **v as u32 == value)
        })
        .map(|(k, _)| k.to_lowercase())
        .unwrap_or_else(|| "unknown".into())
}

fn cache_exports(instance: &Instance, store: &mut Store<Host>, names: &[&str]) -> Result<()> {
    for &name in names {
        let func = instance
            .get_export(&*store, name)
            .and_then(Extern::into_func)
            .ok_or_else(|| anyhow!("missing Ghostty export {name}"))?;
        store.data_mut().exports.insert(name.into(), func);
    }
    Ok(())
}
fn call_untyped(store: &mut Store<Host>, name: &str, args: &[Val]) -> Result<Val> {
    let f = *store
        .data()
        .exports
        .get(name)
        .ok_or_else(|| anyhow!("missing function {name}"))?;
    let count = f.ty(&*store).results().len();
    ensure!(count <= 1, "unexpected multiple results from {name}");
    let mut out = vec![Val::I32(0); count];
    f.call(store, args, &mut out)
        .with_context(|| format!("call {name}"))?;
    Ok(out.first().cloned().unwrap_or(Val::I32(0)))
}
fn result_call(store: &mut Store<Host>, name: &str, args: &[i64]) -> Result<()> {
    let vals: Vec<_> = args.iter().map(|&v| Val::I32(v as i32)).collect();
    let status = val_i32(&call_untyped(store, name, &vals)?).unwrap_or(0);
    ensure!(status == 0, "{name}: Ghostty status {status}");
    Ok(())
}
fn call_u0(store: &mut Store<Host>, name: &str) -> Result<i32> {
    val_i32(&call_untyped(store, name, &[])?).context("invalid return type")
}
fn export_call0(instance: &Instance, store: &mut Store<Host>, name: &str) -> Result<u32> {
    let f = instance
        .get_func(&*store, name)
        .ok_or_else(|| anyhow!("missing callback export {name}"))?;
    let mut out = [Val::I32(0)];
    f.call(store, &[], &mut out)?;
    Ok(val_i32(&out[0]).unwrap_or(-1) as u32)
}
fn alloc(store: &mut Store<Host>, size: u32) -> Result<u32> {
    let ptr = val_i32(&call_untyped(
        store,
        "ghostty_wasm_alloc",
        &[Val::I32(size as i32)],
    )?)
    .unwrap_or(0) as u32;
    ensure!(ptr != 0, "Ghostty allocation failed");
    Ok(ptr)
}
trait DataLike {
    fn func(&self, name: &str) -> Result<Func>;
}
impl DataLike for Host {
    fn func(&self, name: &str) -> Result<Func> {
        self.exports
            .get(name)
            .copied()
            .ok_or_else(|| anyhow!("missing function {name}"))
    }
}
fn call_dynamic<C: AsContextMut>(c: &mut C, name: &str, args: &[Val]) -> Result<Val>
where
    C::Data: DataLike,
{
    let f = c.as_context().data().func(name)?;
    let count = f.ty(c.as_context()).results().len();
    ensure!(count <= 1, "unexpected multiple results from {name}");
    let mut out = vec![Val::I32(0); count];
    f.call(c, args, &mut out)?;
    Ok(out.first().cloned().unwrap_or(Val::I32(0)))
}
fn result_dynamic<C: AsContextMut>(c: &mut C, name: &str, args: &[Val]) -> Result<()>
where
    C::Data: DataLike,
{
    let status = val_i32(&call_dynamic(c, name, args)?).unwrap_or(0);
    ensure!(status == 0, "{name}: Ghostty status {status}");
    Ok(())
}
fn val_i32(value: &Val) -> Option<i32> {
    match value {
        Val::I32(v) => Some(*v),
        _ => None,
    }
}
fn check_memory(store: &Store<Host>) -> Result<()> {
    let Some(memory) = store.data().memory else {
        return Ok(());
    };
    let pages = memory.size(store);
    ensure!(
        pages <= u64::from(MAX_PAGES),
        "Ghostty memory exceeds 64 MiB"
    );
    Ok(())
}
fn read32(store: &Store<Host>, ptr: u32) -> Result<u32> {
    let mut b = [0; 4];
    store
        .data()
        .memory
        .context("missing memory")?
        .read(store, ptr as usize, &mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn dynamic_read32<C: AsContext<Data = Host>>(c: &C, ptr: u32) -> Result<u32> {
    let memory = c.as_context().data().memory.context("missing memory")?;
    let mut bytes = [0; 4];
    memory.read(c, ptr as usize, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}
fn write32(store: &mut Store<Host>, ptr: u32, value: u32) -> Result<()> {
    put32_dynamic(store, ptr, value)
}
fn put_u32_memory<C: AsContextMut<Data = Host>>(c: &mut C, ptr: u32, value: u32) -> Result<()> {
    put32_dynamic(c, ptr, value)
}
fn put32_dynamic<C: AsContextMut<Data = Host>>(c: &mut C, ptr: u32, value: u32) -> Result<()> {
    let memory = c.as_context().data().memory.context("missing memory")?;
    memory.write(c, ptr as usize, &value.to_le_bytes())?;
    Ok(())
}
fn put_u16_memory<C: AsContextMut<Data = Host>>(c: &mut C, ptr: u32, value: u16) -> Result<()> {
    let memory = c.as_context().data().memory.context("missing memory")?;
    memory.write(c, ptr as usize, &value.to_le_bytes())?;
    Ok(())
}
fn write64(store: &mut Store<Host>, ptr: u32, value: u32) -> Result<()> {
    let memory = store.data().memory.context("missing memory")?;
    memory.write(&mut *store, ptr as usize, &u64::from(value).to_le_bytes())?;
    Ok(())
}
fn memory_u32<C: AsContext>(c: &C, m: Memory, ptr: u32) -> Result<u32> {
    let mut b = [0; 4];
    m.read(c, ptr as usize, &mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn memory_u8<C: AsContext>(c: &C, m: Memory, ptr: u32) -> Result<u8> {
    let mut b = [0];
    m.read(c, ptr as usize, &mut b)?;
    Ok(b[0])
}
fn c_string(store: &Store<Host>, ptr: u32, max: u32) -> Result<Vec<u8>> {
    let m = store.data().memory.context("missing memory")?;
    let mut out = Vec::new();
    for i in 0..max {
        let v = memory_u8(store, m, ptr + i)?;
        if v == 0 {
            return Ok(out);
        }
        out.push(v)
    }
    bail!("unterminated Ghostty string")
}
fn abi_string<C: AsContext>(
    c: &C,
    m: Memory,
    abi: &Manifest,
    base: u32,
    ty: &str,
    key: &str,
    max: u32,
) -> Result<String> {
    let member = base + abi.field(ty, key)?.offset;
    let p = memory_u32(c, m, member + abi.field("GhosttyString", "ptr")?.offset)?;
    let n = memory_u32(c, m, member + abi.field("GhosttyString", "len")?.offset)?.min(max);
    let mut b = vec![0; n as usize];
    m.read(c, p as usize, &mut b)?;
    String::from_utf8(b).context("invalid UTF-8 Ghostty metadata")
}
fn pull_string(store: &mut Store<Host>, scratch: u32, key: &str) -> Result<String> {
    let k = store.data().abi.value("GhosttyTerminalData", key)?;
    let terminal = store.data().terminal;
    result_call(
        store,
        "ghostty_terminal_get",
        &[terminal.into(), k.into(), scratch.into()],
    )?;
    let abi = store.data().abi.clone();
    let memory = store.data().memory.context("missing memory")?;
    let p = memory_u32(
        store,
        memory,
        scratch + abi.field("GhosttyString", "ptr")?.offset,
    )?;
    let n = memory_u32(
        store,
        memory,
        scratch + abi.field("GhosttyString", "len")?.offset,
    )?
    .min(4096);
    let mut b = vec![0; n as usize];
    memory.read(store, p as usize, &mut b)?;
    String::from_utf8(b).context("invalid UTF-8 Ghostty metadata")
}

// Add a hard maximum to every defined linear memory before wasmi sees it. This
// preserves an unbounded module's behaviour until the isolation limit and rejects
// at the wasm memory.grow boundary rather than only noticing an excess afterwards.
fn bound_wasm_memories(wasm: &[u8]) -> Result<Vec<u8>> {
    if wasm.len() < 8 || &wasm[..8] != b"\0asm\x01\0\0\0" {
        bail!("invalid Ghostty WASM header");
    }
    let mut out = wasm[..8].to_vec();
    let mut at = 8;
    while at < wasm.len() {
        let id = wasm[at];
        at += 1;
        let (size, n) = uleb(&wasm[at..])?;
        at += n;
        ensure!(size as usize <= wasm.len() - at, "truncated WASM section");
        let original = &wasm[at..at + size as usize];
        let section = if id == 5 {
            bound_memory_section(original)?
        } else {
            original.to_vec()
        };
        out.push(id);
        put_uleb(&mut out, section.len() as u64);
        out.extend_from_slice(&section);
        at += size as usize;
    }
    Ok(out)
}
fn bound_memory_section(data: &[u8]) -> Result<Vec<u8>> {
    let (count, n) = uleb(data)?;
    let mut p = n;
    let mut out = Vec::new();
    put_uleb(&mut out, count);
    for _ in 0..count {
        let (flags, n) = uleb(&data[p..])?;
        p += n;
        ensure!(flags & 4 == 0, "64-bit memories are unsupported");
        let (minimum, n) = uleb(&data[p..])?;
        p += n;
        ensure!(
            minimum <= MAX_PAGES as u64,
            "WASM initial memory exceeds 64 MiB"
        );
        let has_max = flags & 1 != 0;
        let max = if has_max {
            let (v, n) = uleb(&data[p..])?;
            p += n;
            v
        } else {
            MAX_PAGES as u64
        };
        ensure!(max <= MAX_PAGES as u64, "WASM memory exceeds 64 MiB");
        put_uleb(&mut out, flags | 1);
        put_uleb(&mut out, minimum);
        put_uleb(&mut out, max);
        if flags & 4 != 0 {
            bail!("64-bit memories are unsupported");
        }
    }
    ensure!(p == data.len(), "invalid WASM memory section");
    Ok(out)
}
fn put_uleb(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut b = (value & 127) as u8;
        value >>= 7;
        if value != 0 {
            b |= 128;
        }
        out.push(b);
        if value == 0 {
            break;
        }
    }
}
fn uleb(bytes: &[u8]) -> Result<(u64, usize)> {
    let (mut v, mut shift, mut n) = (0u64, 0u32, 0usize);
    for &b in bytes.iter().take(10) {
        v |= ((b & 127) as u64) << shift;
        n += 1;
        if b & 128 == 0 {
            return Ok((v, n));
        }
        shift += 7
    }
    bail!("invalid WASM integer")
}

#[cfg(test)]
mod tests {
    use super::{EXPORT_CSP, Engine, MAX_CAPTURE, sanitize_html};

    #[test]
    fn capture_real_ghostty_wasm_as_text() {
        let wasm = include_bytes!("../web/dist/static/js/ghostty-vt.wasm");
        let engine = Engine::new(wasm).expect("compile Ghostty WASM");
        let mut terminal = engine.terminal(80, 24).expect("instantiate terminal");
        terminal
            .feed(b"hello \xce\xbb\r\n\x1b[31mred <script>\x1b[0m\r\n")
            .expect("feed terminal");
        let captured = terminal
            .capture("text", true, false)
            .expect("capture terminal");
        let text = String::from_utf8(captured).expect("UTF-8 capture");
        assert!(text.contains("hello λ"), "{text:?}");
        assert!(text.contains("red <script>"), "{text:?}");
        let html = terminal
            .capture("html", true, false)
            .expect("safe HTML capture");
        let html = String::from_utf8(html).expect("HTML is UTF-8");
        assert!(html.contains(EXPORT_CSP), "missing export CSP");
        assert!(
            html.contains("red &lt;script&gt;"),
            "capture did not escape tags: {html}"
        );
        assert!(
            !html.contains("red <script>"),
            "unsafe markup escaped from capture"
        );
        assert!(
            html.contains("<div") && html.contains("display: inline"),
            "Ghostty inline formatting was not preserved: {html}"
        );
    }

    #[test]
    fn real_osc_status_and_title_update_metadata() {
        let wasm = include_bytes!("../web/dist/static/js/ghostty-vt.wasm");
        let engine = Engine::new(wasm).expect("compile Ghostty WASM");
        let mut terminal = engine.terminal(80, 24).expect("instantiate terminal");
        terminal
            .feed(b"\x1b]7501;state=working:progress=42:app=test-agent\x07\x1b]0;status-pane\x07")
            .expect("feed OSC metadata");
        let info = terminal.info();
        assert_eq!(info["title"], "status-pane", "title metadata: {info}");
        assert_eq!(
            info["records"][0]["state"], "working",
            "status metadata: {info}"
        );
        assert_eq!(
            info["records"][0]["progress"], 42,
            "progress metadata: {info}"
        );
        assert_eq!(
            info["records"][0]["app"], "test-agent",
            "app metadata: {info}"
        );
        assert_eq!(info["state"], "working", "summary state: {info}");
        assert_eq!(info["progress"], 42, "summary progress: {info}");
        assert_eq!(
            info["program_status"]["records"][0]["state"], "working",
            "native status response: {info}"
        );
        let replies = terminal
            .feed(b"\x1b[6n")
            .expect("terminal query remains consumable");
        assert!(!replies.is_empty(), "query reply was lost: {replies:?}");
    }

    #[test]
    fn html_sanitizer_strips_active_content_and_preserves_terminal_styles() {
        let source = br#"<html><head><title>terminal</title></head><body><div style="font-family: monospace; white-space: pre; background-color: #112233; position: fixed; color: expression(alert(1))"><span style="color: rgb(1, 2, 3); font-weight: bold" onclick="bad()">red</span><script>alert(1)</script><iframe src="https://bad.invalid">hidden</iframe><a href="javascript:alert(1)">bad link</a><a href="https://example.test/a">safe link</a><img src="x" onerror="bad()"><base href="https://bad.invalid"></div></body></html>"#;
        let html = String::from_utf8(sanitize_html(source).expect("sanitize fixture")).unwrap();
        assert!(html.contains(EXPORT_CSP), "missing CSP: {html}");
        assert!(
            html.contains("font-family: monospace"),
            "lost safe CSS: {html}"
        );
        assert!(
            html.contains("background-color: #112233"),
            "lost safe color: {html}"
        );
        assert!(
            html.contains("color: rgb(1, 2, 3)"),
            "lost span color: {html}"
        );
        for forbidden in [
            "position: fixed",
            "expression(",
            "onclick",
            "onerror",
            "<script",
            "<iframe",
            "hidden</iframe>",
            "javascript:",
            "<img",
            "<base",
        ] {
            assert!(
                !html.contains(forbidden),
                "unsafe content {forbidden:?}: {html}"
            );
        }
        assert!(html.contains("bad link"), "link text should remain: {html}");
        assert!(
            html.contains("href=\"https://example.test/a\""),
            "safe hyperlink removed: {html}"
        );
    }

    #[test]
    fn html_sanitizer_rejects_encoded_schemes_and_oversize_input() {
        let encoded = b"<a href=\"java&#x73;cript:alert(1)\">link</a>";
        let html =
            String::from_utf8(sanitize_html(encoded).expect("sanitize encoded URL")).unwrap();
        assert!(!html.contains("href="), "encoded scheme passed: {html}");
        assert!(html.contains("link"), "text disappeared: {html}");

        let oversized = vec![b'x'; MAX_CAPTURE as usize + 1];
        assert!(
            sanitize_html(&oversized).is_err(),
            "oversize input accepted"
        );
    }
}
