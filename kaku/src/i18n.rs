//! External overlay language pack for CLI/TUI and GUI display strings.
//!
//! UI strings stay as English literals in the source. A flat JSON file at
//! `<config_dir>/lang/<locale>.json` maps the exact English original text to
//! a translation; lookups that miss fall back to the English original, so a
//! raw key can never render (the failure mode that doomed PR #362, issue
//! #391). The pack is read once per process — when a TUI starts, and in
//! kaku-gui before the first menubar build; edits take effect on restart.
//! GUI consumers (menubar titles, context menus, toasts, command palette /
//! launcher labels) translate at the display seam only: AppKit menu items
//! get the translated title at creation, and anything persisted or compared
//! with `==` (command `brief` as the frecency key) stays English.
//!
//! Never translate persistent values (config values written back to
//! kaku.lua, strings compared with `==`), machine-readable CLI output, or
//! files mirrored from upstream `wezterm/src/cli`.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, RwLock};

/// In-process lookup misses, capped so a long session cannot grow it without
/// bound. Surfaced by `kaku doctor` (Phase 1.5) and `KAKU_I18N_TRACE=1`.
/// The set deduplicates; the Vec keeps insertion order for reporting.
const MISS_CAP: usize = 500;

struct MissLog {
    seen: HashSet<String>,
    ordered: Vec<String>,
}

impl MissLog {
    fn new() -> Self {
        Self {
            seen: HashSet::new(),
            ordered: Vec::new(),
        }
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.seen.clear();
        self.ordered.clear();
    }
}

fn misses() -> &'static Mutex<MissLog> {
    static MISSES: OnceLock<Mutex<MissLog>> = OnceLock::new();
    MISSES.get_or_init(|| Mutex::new(MissLog::new()))
}

fn table() -> &'static RwLock<HashMap<String, String>> {
    static TABLE: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
    TABLE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Resolve `<config_dir>/lang/<locale>.json` for the active locale. `None`
/// when the locale is English (or unset) — no file is read, the table stays
/// empty, and every surface renders exactly as before.
fn pack_path() -> Option<PathBuf> {
    let locale = active_locale()?;
    if locale == "en" {
        return None;
    }
    // Follow the config file Kaku actually loaded (`--config-file`,
    // `KAKU_CONFIG_FILE`, then the default user path), so a user whose
    // config lives elsewhere finds their pack next to it. The pack lives
    // in a sibling `lang/` directory, same "repo-external optional file"
    // pattern as assistant.toml.
    let dir = config::effective_config_file_path().parent()?.join("lang");
    Some(dir.join(format!("{locale}.json")))
}

/// `KAKU_LANG` > `LC_ALL` > `LC_MESSAGES` > `LANG`, reduced to the primary
/// subtag (`zh_CN.UTF-8` -> `zh`). An explicit `KAKU_LANG=en` forces English.
fn active_locale() -> Option<String> {
    for var in ["KAKU_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
        let Ok(value) = std::env::var(var) else {
            continue;
        };
        match resolve_locale(value.trim()) {
            Locale::C => return None,
            Locale::Language(code) => return Some(code),
            // A set-but-unparseable value stops the search: the user set
            // this variable deliberately, falling through would override it.
            Locale::Unknown => return None,
        }
    }
    None
}

/// What one locale-variable value resolves to.
#[derive(Debug, PartialEq)]
enum Locale {
    /// C/POSIX (with or without codeset): an explicit "no localization".
    C,
    /// A usable primary language subtag, lowercased.
    Language(String),
    /// Set but not parseable as a locale.
    Unknown,
}

fn resolve_locale(value: &str) -> Locale {
    let locale_part = value.split(['.', '@']).next().unwrap_or(value);
    if is_c_locale(locale_part) {
        return Locale::C;
    }
    // Only the primary language subtag (before the first `-` or `_`)
    // selects the pack file: zh_CN.UTF-8 -> zh, zh-Hans -> zh.
    match locale_part.split(['-', '_']).next().map(str::trim) {
        Some(primary) if !primary.is_empty() => Locale::Language(primary.to_ascii_lowercase()),
        _ => Locale::Unknown,
    }
}

/// Read and install the language pack. Missing file, parse error, or
/// permission error all leave an empty table; none of them can fail any
/// other flow. Idempotent: TUI `run()` entry points may call it again.
pub fn load() {
    let map = match read_pack() {
        Some(map) => map,
        None => {
            HAS_PACK.store(false, std::sync::atomic::Ordering::Relaxed);
            *table().write().unwrap_or_else(|e| e.into_inner()) = HashMap::new();
            return;
        }
    };
    HAS_PACK.store(!map.is_empty(), std::sync::atomic::Ordering::Relaxed);
    *table().write().unwrap_or_else(|e| e.into_inner()) = map;
}

/// Read the pack file and return its usable entries. `None` when there is
/// no pack (or it cannot be read); a broken pack logs why, because a
/// silently empty table looks exactly like "the pack was never written".
fn read_pack() -> Option<HashMap<String, String>> {
    let path = pack_path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            log::warn!("cannot read language pack {}: {}", path.display(), e);
            return None;
        }
    };
    let raw: HashMap<String, serde_json::Value> = match serde_json::from_str(&text) {
        Ok(raw) => raw,
        Err(e) => {
            log::warn!("language pack {} is not valid JSON: {}", path.display(), e);
            return None;
        }
    };
    let map: HashMap<String, String> = raw
        .into_iter()
        .filter(|(key, value)| valid_entry(key, value))
        .map(|(key, value)| {
            (
                key,
                value
                    .as_str()
                    .expect("valid_entry checked this")
                    .to_string(),
            )
        })
        .collect();
    if map.is_empty() {
        log::warn!(
            "language pack {} has no usable entries (keys starting with `_` are ignored)",
            path.display()
        );
    }
    Some(map)
}

/// True once a non-empty pack is installed. The hot path (tr() on every
/// label of every frame) checks this before paying for miss bookkeeping.
static HAS_PACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn valid_entry(key: &str, value: &serde_json::Value) -> bool {
    !key.starts_with('_') && value.as_str().is_some_and(|s| !s.trim().is_empty())
}

/// True when a locale value denotes the C/POSIX locale, including the
/// codeset-carrying `C.UTF-8` form. Production rule shared by
/// `active_locale` and its tests.
fn is_c_locale(value: &str) -> bool {
    let locale_part = value.split(['.', '@']).next().unwrap_or(value);
    locale_part.eq_ignore_ascii_case("C") || locale_part.eq_ignore_ascii_case("POSIX")
}

/// Exact-match lookup. A hit returns the translation, a miss returns the
/// English original (borrowed). With no pack loaded every surface renders
/// exactly as before: `tr` short-circuits on HAS_PACK and never takes the
/// table lock, hashes, or records a miss.
///
/// Lock discipline: the table guard is dropped BEFORE record_miss() runs.
/// record_miss() never touches the table lock, so a queued load() writer
/// can never deadlock a render frame (std RwLock is write-preferring: a
/// nested read under a held read guard blocks while a writer queues).
pub fn tr(s: &str) -> Cow<'_, str> {
    if !HAS_PACK.load(std::sync::atomic::Ordering::Relaxed) {
        return Cow::Borrowed(s);
    }
    let lookup = {
        let guard = table().read().unwrap_or_else(|e| e.into_inner());
        guard.get(s).cloned()
    };
    match lookup {
        Some(v) => Cow::Owned(v),
        None => {
            record_miss(s);
            Cow::Borrowed(s)
        }
    }
}

/// Dynamic templates: only bare `{}` placeholders are translatable. A
/// translation whose `{}` count disagrees with the arguments falls back to
/// the English template. Argument text is never looked up recursively.
/// Used for status/error copy in the AI config TUI; Phase 1.5 extends this
/// to doctor/update/init println copy.
pub fn trf(tmpl: &str, args: &[&dyn Display]) -> String {
    if !HAS_PACK.load(std::sync::atomic::Ordering::Relaxed) {
        return render_bare_or_verbatim(tmpl, args);
    }
    if let Some(translation) = table()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(tmpl)
        .cloned()
    {
        if let Some(rendered) = render_bare(&translation, args) {
            return rendered;
        }
    }
    // 模板本身没有译文：把模板原文（不是渲染后的句子）记为缺失项，
    // 这样 doctor 看到的永远是可翻译的模板 key
    record_miss(tmpl);
    render_bare_or_verbatim(tmpl, args)
}

/// A translated (or original) template that render_bare rejects must
/// never swallow the message: fall back to splicing the arguments into
/// the English template, and only give up — on the English template's
/// own guardrail failure — by returning it verbatim with args appended,
/// still never an empty string.
fn render_bare_or_verbatim(tmpl: &str, args: &[&dyn Display]) -> String {
    render_bare(tmpl, args).unwrap_or_else(|| {
        let mut out = tmpl.to_string();
        for arg in args {
            out.push(' ');
            out.push_str(&arg.to_string());
        }
        out
    })
}

/// Convenience wrapper: `trf!("Saved {} as {}", key, value)` expands to the
/// `trf` function with the argument list boxed as `&dyn Display`.
#[macro_export]
macro_rules! trf {
    ($tmpl:expr $(, $arg:expr)* $(,)?) => {
        $crate::i18n::trf($tmpl, &[$(&$arg as &dyn std::fmt::Display),*])
    };
}

/// Render a bare-`{}` template. Anything with `{{`, named placeholders, or a
/// placeholder-count mismatch returns `None` (untranslatable guardrail) —
/// such templates keep their native `format!` and must never enter the pack.
fn render_bare(tmpl: &str, args: &[&dyn Display]) -> Option<String> {
    if tmpl.contains("{{") || tmpl.matches('{').count() != tmpl.matches("{}").count() {
        return None;
    }
    if tmpl.matches("{}").count() != args.len() {
        return None;
    }
    let mut out = String::with_capacity(tmpl.len() + 32);
    let mut parts = tmpl.split("{}");
    out.push_str(parts.next().unwrap_or(""));
    for (arg, tail) in args.iter().zip(parts) {
        out.push_str(&arg.to_string());
        out.push_str(tail);
    }
    Some(out)
}

fn record_miss(s: &str) {
    // No pack loaded: nothing can ever translate, skip all bookkeeping.
    // This is the pure-English hot path (every label misses); the atomic
    // read keeps it allocation- and lock-free.
    if !HAS_PACK.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    // Cheap gate first: the trace flag is read once per process, never on
    // the per-frame hot path.
    static TRACE: OnceLock<bool> = OnceLock::new();
    let trace = *TRACE.get_or_init(|| std::env::var_os("KAKU_I18N_TRACE").is_some());
    let mut misses = misses().lock().unwrap_or_else(|e| e.into_inner());
    if misses.seen.contains(s) {
        return; // already known: no re-record, no re-trace
    }
    if misses.ordered.len() >= MISS_CAP {
        return;
    }
    misses.seen.insert(s.to_string());
    misses.ordered.push(s.to_string());
    // Trace only on first sight of a miss: the TUI redraws at frame rate
    // and re-printing every frame would bury the terminal.
    if trace {
        eprintln!("[i18n] missing entry: {s:?}");
    }
}

/// Missed originals so far this process, for doctor reporting.
/// Phase 1.5 wires this into `kaku doctor`; kept public so the
/// doctor work is a pure addition.
#[allow(dead_code)]
pub fn recorded_misses() -> Vec<String> {
    misses()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .ordered
        .clone()
}

/// Display width of a string (CJK counts double), replacing `chars().count()`
/// padding that misaligns translated labels.
pub fn display_width(s: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(s)
}

/// Right-pad with spaces to at least `width` display columns.
pub fn pad_end_width(s: &str, width: usize) -> String {
    let current = display_width(s);
    if current >= width {
        s.to_string()
    } else {
        let pad = width - current;
        let mut out = String::with_capacity(s.len() + pad);
        out.push_str(s);
        out.extend(std::iter::repeat_n(' ', pad));
        out
    }
}

/// Truncate to at most `width` display columns. `pad_end_width` never
/// truncates, so a column width cap only holds if the label itself is
/// cut; this keeps a long translation inside its column instead of
/// pushing the value off screen.
pub fn truncate_width(s: &str, width: usize) -> Cow<'_, str> {
    if display_width(s) <= width {
        return Cow::Borrowed(s);
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table, the miss list and the env are process-global; `cargo test`
    /// runs tests in one process on many threads and they would stomp each
    /// other's table. nextest (what `make test` runs) isolates per test
    /// process, so this lock only matters for plain `cargo test` runs.
    static TABLE_LOCK: Mutex<()> = Mutex::new(());
    use std::time::Duration;

    #[test]
    fn locale_resolution_reduces_to_primary_subtag() {
        // The production parser, exercised directly.
        assert_eq!(
            resolve_locale("zh_CN.UTF-8"),
            Locale::Language("zh".to_string())
        );
        assert_eq!(
            resolve_locale("zh-Hans@pinyin"),
            Locale::Language("zh".to_string())
        );
        assert_eq!(resolve_locale("en_US"), Locale::Language("en".to_string()));
        assert_eq!(resolve_locale(""), Locale::Unknown);
        assert_eq!(resolve_locale("  "), Locale::Unknown);
    }

    /// LC_ALL=C must beat LANG=zh (stop, not fall through), and C.UTF-8
    /// must resolve to no locale rather than a "c" pack.
    #[test]
    fn c_locale_stops_the_search() {
        assert_eq!(resolve_locale("C"), Locale::C);
        assert_eq!(resolve_locale("POSIX"), Locale::C);
        assert_eq!(resolve_locale("C.UTF-8"), Locale::C);
        assert_eq!(resolve_locale("c"), Locale::C);
        assert_ne!(resolve_locale("zh_CN.UTF-8"), Locale::C);
        assert_ne!(resolve_locale("en_US"), Locale::C);
    }

    #[test]
    fn tr_miss_falls_back_to_original_and_records_once() {
        let _guard = TABLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Install a one-entry table directly: the public load() reads the
        // real user config dir, which a test must not depend on. The
        // HAS_PACK gate must be flipped the way load() does.
        *table().write().unwrap_or_else(|e| e.into_inner()) =
            HashMap::from([("Settings".to_string(), "设置".to_string())]);
        HAS_PACK.store(true, std::sync::atomic::Ordering::Relaxed);
        misses().lock().unwrap_or_else(|e| e.into_inner()).clear();

        assert_eq!(tr("Settings"), "设置");
        // A miss falls back to the exact original: no bare keys, ever.
        assert_eq!(tr("Appearance"), "Appearance");
        let recorded = recorded_misses();
        assert_eq!(recorded, vec!["Appearance".to_string()]);
        // The same miss is not recorded twice.
        let _ = tr("Appearance");
        assert_eq!(recorded_misses().len(), 1);

        // With the gate off (no pack), misses are not recorded at all.
        HAS_PACK.store(false, std::sync::atomic::Ordering::Relaxed);
        misses().lock().unwrap_or_else(|e| e.into_inner()).clear();
        let _ = tr("Something else");
        assert!(recorded_misses().is_empty());

        *table().write().unwrap_or_else(|e| e.into_inner()) = HashMap::new();
    }

    /// Regression: `tr()` must not deadlock when a writer queues on the
    /// table. The old implementation took the table read lock a second
    /// time while still holding the first guard; std's write-preferring
    /// RwLock blocks that nested read for as long as the writer waits,
    /// and the TUI render loop hangs forever.
    ///
    /// The writer here holds the lock for 50 ms, so `tr()` legitimately
    /// waits for it — the invariant is that `tr()` *returns*, not that it
    /// returns before the writer is done. Asserting the timing instead
    /// made this test pass or fail depending on when the writer thread
    /// got scheduled.
    #[test]
    fn tr_completes_while_a_writer_holds_the_table() {
        let _guard = TABLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *table().write().unwrap_or_else(|e| e.into_inner()) =
            HashMap::from([("Settings".to_string(), "设置".to_string())]);
        misses().lock().unwrap_or_else(|e| e.into_inner()).clear();

        let writer = std::thread::spawn(|| {
            let guard = table().write().unwrap_or_else(|e| e.into_inner());
            std::thread::sleep(Duration::from_millis(50));
            drop(guard);
        });
        std::thread::sleep(Duration::from_millis(10)); // writer has the lock
        let (tx, rx) = std::sync::mpsc::channel();
        let lookup = std::thread::spawn(move || {
            let _ = tx.send(std::hint::black_box(tr("Appearance")));
        });
        assert!(
            rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "tr() did not return while a writer was queued"
        );
        writer.join().unwrap();
        lookup.join().unwrap();
        misses().lock().unwrap_or_else(|e| e.into_inner()).clear();
        *table().write().unwrap_or_else(|e| e.into_inner()) = HashMap::new();
    }

    #[test]
    fn trf_translates_bare_templates_and_rejects_mismatched_ones() {
        let _guard = TABLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *table().write().unwrap_or_else(|e| e.into_inner()) = HashMap::from([
            (
                "Current version: {}".to_string(),
                "当前版本：{}".to_string(),
            ),
            (
                "Downloading {} ...".to_string(),
                "正在下载 {} ...".to_string(),
            ),
            // A bad entry whose placeholder count disagrees with the
            // original must be skipped, not rendered.
            ("Saved {} as {}".to_string(), "已保存 {}".to_string()),
        ]);
        // trf() short-circuits on HAS_PACK like tr() does; flip the gate the
        // way load() does.
        HAS_PACK.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(trf("Current version: {}", &[&"0.28.0"]), "当前版本：0.28.0");
        assert_eq!(
            trf("Downloading {} ...", &[&"kaku.zip"]),
            "正在下载 kaku.zip ..."
        );
        // Placeholder mismatch in the translation falls back to English.
        assert_eq!(
            trf("Saved {} as {}", &[&"Font", &"JetBrains Mono"]),
            "Saved Font as JetBrains Mono"
        );
        // A template never seen in the pack still renders in English.
        assert_eq!(trf("Unknown {}", &[&"x"]), "Unknown x");
        // A guardrailed (non-bare) English template must never produce an
        // empty message: the template text survives with the args appended.
        assert_eq!(trf("Hello {name}", &[&"x"]), "Hello {name} x");
        assert_eq!(trf("a {{b}} {}", &[&"x"]), "a {{b}} {} x");
        HAS_PACK.store(false, std::sync::atomic::Ordering::Relaxed);
        *table().write().unwrap_or_else(|e| e.into_inner()) = HashMap::new();
    }

    /// 语言包跟着实际加载的配置文件走：`lang/<locale>.json` 就放在那个
    /// 配置文件旁边，`--config-file` 指向别处时也成立。
    #[test]
    fn pack_path_follows_the_effective_config_file() {
        let _guard = TABLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_lang = std::env::var("KAKU_LANG").ok();
        std::env::set_var("KAKU_LANG", "zh_CN.UTF-8");
        let expected = config::effective_config_file_path()
            .parent()
            .map(|dir| dir.join("lang").join("zh.json"));
        assert_eq!(pack_path(), expected);
        match prev_lang {
            Some(value) => std::env::set_var("KAKU_LANG", value),
            None => std::env::remove_var("KAKU_LANG"),
        }
    }

    #[test]
    fn render_bare_guardrails() {
        // Escaped braces, named placeholders and wrong arity are all
        // untranslatable: None keeps them out of the pack's reach.
        assert_eq!(render_bare("a {{b}} {}", &[&"x"]), None);
        assert_eq!(render_bare("a {name} {}", &[&"x"]), None);
        assert_eq!(render_bare("a {} {}", &[&"x"]), None);
        assert_eq!(
            render_bare("a {} b {}", &[&"x", &"y"]),
            Some("a x b y".into())
        );
    }

    #[test]
    fn display_width_counts_cjk_double_and_pads() {
        assert_eq!(display_width("Settings"), 8);
        assert_eq!(display_width("设置"), 4);
        assert_eq!(pad_end_width("设置", 8), "设置    ");
        assert_eq!(pad_end_width("Settings", 8), "Settings");
        assert_eq!(pad_end_width("longer than width", 8), "longer than width");
    }

    /// 列宽上限必须真的生效：`pad_end_width` 不截断，超长译文要靠
    /// truncate_width 切掉，否则会把值挤出屏幕。
    #[test]
    fn truncate_width_cuts_to_the_column_bound() {
        assert_eq!(truncate_width("Settings", 8), "Settings");
        assert_eq!(truncate_width("Settings", 4), "Sett");
        assert_eq!(display_width(&truncate_width("基础设置", 4)), 4);
        assert_eq!(truncate_width("设置", 4), "设置");
    }

    /// 模板缺译文时，记录的是模板原文（可翻译的 key），不是渲染后的
    /// 句子（带着运行时数据）。
    #[test]
    fn trf_records_the_missing_template_key() {
        let _guard = TABLE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        *table().write().unwrap_or_else(|e| e.into_inner()) =
            HashMap::from([("Settings".to_string(), "设置".to_string())]);
        misses().lock().unwrap_or_else(|e| e.into_inner()).clear();
        HAS_PACK.store(true, std::sync::atomic::Ordering::Relaxed);

        assert_eq!(
            trf("Saved {} as {}", &[&"Font", &"Mono"]),
            "Saved Font as Mono"
        );
        let recorded = recorded_misses();
        assert_eq!(recorded, vec!["Saved {} as {}".to_string()]);

        HAS_PACK.store(false, std::sync::atomic::Ordering::Relaxed);
        misses().lock().unwrap_or_else(|e| e.into_inner()).clear();
        *table().write().unwrap_or_else(|e| e.into_inner()) = HashMap::new();
    }

    #[test]
    fn load_ignores_invalid_entries() {
        use tempfile::TempDir;
        let dir = TempDir::new().unwrap();
        let lang = dir.path().join("lang");
        std::fs::create_dir(&lang).unwrap();
        let pack = lang.join("zh.json");
        std::fs::write(
            &pack,
            r#"{
  "_note": "ignored",
  "Settings": "设置",
  "Empty": "",
  "Blank": "   ",
  "Not a string": 42
}"#,
        )
        .unwrap();

        let map: HashMap<String, String> =
            serde_json::from_str::<HashMap<String, serde_json::Value>>(
                &std::fs::read_to_string(&pack).unwrap(),
            )
            .unwrap()
            .into_iter()
            .filter(|(k, v)| valid_entry(k, v))
            .map(|(k, v)| (k, v.as_str().unwrap().to_string()))
            .collect();
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("Settings").map(String::as_str), Some("设置"));
    }
}
