//! The user's shairport-sync.conf.
//!
//! shairport-sync removes the comments from its config, expands ${NAME}
//! references and parses the rest with libconfig. The settings the app reads
//! and writes are found with a small parser for libconfig's syntax, so they can
//! be laid out in any way libconfig accepts, and a write changes only the text
//! of the one setting it sets.
//!
//! libconfig takes any bytes, so the file is handled as bytes throughout:
//! bytes that aren't UTF-8 are carried through unchanged instead of being an
//! error.

use std::ffi::{CString, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// The pipewire.sink_target line as shipped (commented out = follow the system default).
pub const SINK_TARGET_DEFAULT_LINE: &str = "//\tsink_target = \"<sink target name>\"; // Leave this commented out \
     to get the sink target already chosen by the PipeWire system.\n";
/// Starts the comment after a sink_target written by the app, which names the output.
pub const LABEL_MARKER: &str = "AirPlay Receiver: ";

// pipewire.mixer_type = "sink" makes the sender's volume control set the output
// device's own volume (its hardware mixer where it has one) instead of
// shairport-sync attenuating the audio in software. The stock config has no
// mixer_type line, so "off" is written as a commented-out line.
pub const MIXER_TYPE_ON_COMMENT: &str =
    "AirPlay Receiver: the sender's volume control sets the output device's volume.";
pub const MIXER_TYPE_OFF_LINE: &str = "//\tmixer_type = \"sink\"; // Set to \"sink\" to make the sender's volume control \
     set the output device's volume instead of attenuating the audio in software.\n";

/// How many nested parse calls are allowed before a file counts as nested too
/// deeply (two per group, one per list or array).
const MAX_DEPTH: usize = 900;

/// The config file can't be changed safely, e.g. because it doesn't parse.
#[derive(Debug)]
pub enum ConfigError {
    /// The file can't be changed safely as it is.
    Unsafe(String),
    /// Reading or writing `path` failed.
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Unsafe(message) => f.write_str(message),
            ConfigError::Io { path, source } => write!(f, "{}: {}", path.display(), source),
        }
    }
}

impl std::error::Error for ConfigError {}

fn io_error(path: &Path) -> impl FnOnce(io::Error) -> ConfigError + '_ {
    move |source| ConfigError::Io { path: path.to_path_buf(), source }
}

/// Where libconfig would reject the file's structure, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub offset: usize,
    pub message: &'static str,
}

/// Where the settings of a group (or of the whole file) are.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Group {
    /// Offset of the "{" (0 for the file).
    pub start: usize,
    /// For the file, the offset its settings end at (see `tokenize`).
    pub end: Option<usize>,
    /// The settings in the order they appear, by name.
    pub settings: Vec<(Vec<u8>, Setting)>,
    /// (start, end) of each comment between the settings.
    pub comments: Vec<(usize, usize)>,
}

impl Group {
    pub fn get(&self, name: &[u8]) -> Option<&Setting> {
        self.settings.iter().find(|(n, _)| n == name).map(|(_, s)| s)
    }
}

/// Where a `name = value;` is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Setting {
    /// Offset of the name.
    pub start: usize,
    /// Offset just past the value, or past its ";" or ",".
    pub end: usize,
    /// (start, end) of each quoted piece of a string value.
    pub strings: Option<Vec<(usize, usize)>>,
    /// The group of a group value.
    pub group: Option<Group>,
}

// ---- tokens ---------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Comment,
    OpenComment,
    Str,
    OpenString,
    Env,
    Include,
    Name,
    Number,
    Punct,
    Other,
    End,
}

#[derive(Debug, Clone, Copy)]
struct Token {
    kind: Kind,
    start: usize,
    end: usize,
}

fn is_blank(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | 0x0c)
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\n' | 0x0c)
}

fn skip_blanks(text: &[u8], mut i: usize) -> usize {
    while i < text.len() && is_blank(text[i]) {
        i += 1;
    }
    i
}

fn skip_digits(text: &[u8], mut i: usize) -> usize {
    while i < text.len() && text[i].is_ascii_digit() {
        i += 1;
    }
    i
}

fn find(text: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > text.len() {
        return None;
    }
    text[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// Whether a word character (a letter, a digit or "_", in any script) starts at `i`.
fn word_char_at(text: &[u8], i: usize) -> bool {
    match text.get(i) {
        None => false,
        Some(&b) if b < 0x80 => b.is_ascii_alphanumeric() || b == b'_',
        Some(_) => (2..=4)
            .filter_map(|n| text.get(i..i + n))
            .find_map(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|s| s.chars().next())
            .is_some_and(char::is_alphanumeric),
    }
}

/// `${NAME}` at `i`: the offset just past it.
fn env_ref_end(text: &[u8], i: usize) -> Option<usize> {
    if !text[i..].starts_with(b"${") {
        return None;
    }
    let mut j = i + 2;
    if !text.get(j).is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_') {
        return None;
    }
    while text.get(j).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') {
        j += 1;
    }
    (text.get(j) == Some(&b'}')).then_some(j + 1)
}

/// `[eE][-+]?[0-9]+` at `i`: the offset just past it.
fn exponent_end(text: &[u8], i: usize) -> Option<usize> {
    if !matches!(text.get(i), Some(b'e' | b'E')) {
        return None;
    }
    let mut j = i + 1;
    if matches!(text.get(j), Some(b'-' | b'+')) {
        j += 1;
    }
    let end = skip_digits(text, j);
    (end > j).then_some(end)
}

fn skip_long_suffix(text: &[u8], mut i: usize) -> usize {
    for _ in 0..2 {
        if text.get(i) == Some(&b'L') {
            i += 1;
        }
    }
    i
}

/// A libconfig number at `i`: the offset just past it.
fn number_end(text: &[u8], i: usize) -> Option<usize> {
    if text[i] == b'0' && matches!(text.get(i + 1), Some(b'x' | b'X')) {
        let mut j = i + 2;
        while text.get(j).is_some_and(u8::is_ascii_hexdigit) {
            j += 1;
        }
        if j > i + 2 {
            return Some(skip_long_suffix(text, j));
        }
    }
    let start = if matches!(text[i], b'-' | b'+') { i + 1 } else { i };
    let digits = skip_digits(text, start);
    if text.get(digits) == Some(&b'.') {
        let fraction = skip_digits(text, digits + 1);
        return Some(exponent_end(text, fraction).unwrap_or(fraction));
    }
    if digits == start {
        return None;
    }
    Some(exponent_end(text, digits).unwrap_or_else(|| skip_long_suffix(text, digits)))
}

/// The token at `i`.
fn token_at(text: &[u8], i: usize) -> Token {
    let len = text.len();
    let rest = &text[i..];
    let token = |kind, end| Token { kind, start: i, end };
    let b = text[i];

    if is_space(b) {
        let mut j = i;
        while j < len && is_space(text[j]) {
            j += 1;
        }
        // Spaces are dropped by the caller; Other is never used for them.
        return Token { kind: Kind::End, start: i, end: j };
    }
    if rest.starts_with(b"//") || b == b'#' {
        let end = find(text, b"\n", i).unwrap_or(len);
        return token(Kind::Comment, end);
    }
    if rest.starts_with(b"/*") {
        return match find(text, b"*/", i + 2) {
            Some(close) => token(Kind::Comment, close + 2),
            None => token(Kind::OpenComment, len),
        };
    }
    if b == b'"' {
        let mut j = i + 1;
        loop {
            match text.get(j) {
                Some(b'"') => return token(Kind::Str, j + 1),
                Some(b'\\') if j + 1 < len => j += 2,
                Some(b'\\') | None => return token(Kind::OpenString, len),
                Some(_) => j += 1,
            }
        }
    }
    if let Some(end) = env_ref_end(text, i) {
        return token(Kind::Env, end);
    }
    if rest.starts_with(b"@include") && !word_char_at(text, i + 8) {
        return token(Kind::Include, i + 8);
    }
    if b.is_ascii_alphabetic() || b == b'*' {
        let mut j = i + 1;
        while text.get(j).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'*')) {
            j += 1;
        }
        return token(Kind::Name, j);
    }
    if let Some(end) = number_end(text, i) {
        return token(Kind::Number, end);
    }
    if b"=:;,{}()[]".contains(&b) {
        return token(Kind::Punct, i + 1);
    }
    token(Kind::Other, i + 1)
}

/// libconfig's tokens, without the white space. Comments are kept: one can
/// hold the label of an output, or be a commented-out setting for a new
/// setting to replace. A string or a /* comment still open at the end of the
/// file is dropped, as libconfig and shairport-sync do, so the file's settings
/// end where it starts; it is returned apart. A ${NAME} outside a string is
/// replaced by the variable's text before libconfig sees it, so it is taken as
/// a value of some kind (one the app doesn't read).
fn tokenize(text: &[u8]) -> (Vec<Token>, Option<Token>) {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let token = token_at(text, i);
        i = token.end;
        if !is_space(text[token.start]) {
            tokens.push(token);
        }
    }
    let left_open = match tokens.last() {
        Some(last) if matches!(last.kind, Kind::OpenString | Kind::OpenComment) => tokens.pop(),
        _ => None,
    };
    tokens.push(Token { kind: Kind::End, start: text.len(), end: text.len() });
    (tokens, left_open)
}

// ---- parser ---------------------------------------------------------------

struct Parser<'a> {
    text: &'a [u8],
    tokens: Vec<Token>,
    open_string: Option<usize>,
    depth: usize,
}

type Parsed<T> = Result<T, ParseError>;

impl Parser<'_> {
    fn fail(&self, i: usize, message: &'static str) -> ParseError {
        let token = self.tokens[i];
        if token.kind == Kind::End {
            return match self.open_string {
                Some(offset) => ParseError { offset, message: "unterminated string" },
                None => ParseError { offset: token.start, message: "unexpected end of file" },
            };
        }
        ParseError { offset: token.start, message }
    }

    fn syntax_error(&self, i: usize) -> ParseError {
        self.fail(i, "syntax error")
    }

    fn is_punct(&self, i: usize, chars: &[u8]) -> bool {
        let token = self.tokens[i];
        token.kind == Kind::Punct && chars.contains(&self.text[token.start])
    }

    fn word(&self, i: usize) -> &[u8] {
        &self.text[self.tokens[i].start..self.tokens[i].end]
    }

    fn is_boolean(&self, i: usize) -> bool {
        let word = self.word(i);
        word.eq_ignore_ascii_case(b"true") || word.eq_ignore_ascii_case(b"false")
    }

    fn skip_comments(&self, mut i: usize, mut comments: Option<&mut Vec<(usize, usize)>>) -> usize {
        while self.tokens[i].kind == Kind::Comment {
            if let Some(comments) = comments.as_deref_mut() {
                comments.push((self.tokens[i].start, self.tokens[i].end));
            }
            i += 1;
        }
        i
    }

    fn enter(&mut self) -> Parsed<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(ParseError { offset: 0, message: "groups nested too deeply" });
        }
        Ok(())
    }

    /// Up to the closing "}", or with closing false, to the end of the file.
    fn parse_settings(&mut self, mut i: usize, group: &mut Group, closing: bool) -> Parsed<usize> {
        self.enter()?;
        loop {
            i = self.skip_comments(i, Some(&mut group.comments));
            let token = self.tokens[i];
            if if closing { self.is_punct(i, b"}") } else { token.kind == Kind::End } {
                self.depth -= 1;
                return Ok(i + 1);
            }
            if token.kind == Kind::Include {
                // The included file's settings can't be seen from here.
                return Err(self.fail(i, "@include isn't supported"));
            }
            if token.kind == Kind::Env {
                return Err(self.fail(i, "a setting name from an environment variable isn't supported"));
            }
            if token.kind != Kind::Name || self.is_boolean(i) {
                return Err(self.syntax_error(i));
            }
            let name = self.word(i).to_vec();
            if group.get(&name).is_some() {
                return Err(self.fail(i, "duplicate setting name"));
            }
            let mut setting = Setting { start: token.start, end: token.start, strings: None, group: None };
            i = self.skip_comments(i + 1, None);
            if !self.is_punct(i, b"=:") {
                return Err(self.syntax_error(i));
            }
            let value = self.skip_comments(i + 1, None);
            i = self.parse_value(value, Some(&mut setting))?;
            setting.end = self.tokens[i - 1].end;
            let terminator = self.skip_comments(i, None);
            if self.is_punct(terminator, b";,") {
                i = terminator + 1;
                setting.end = self.tokens[terminator].end;
            }
            group.settings.push((name, setting));
        }
    }

    fn parse_value(&mut self, mut i: usize, setting: Option<&mut Setting>) -> Parsed<usize> {
        self.enter()?;
        let token = self.tokens[i];
        let next = match token.kind {
            Kind::Str => {
                let mut pieces = vec![(token.start, token.end)];
                loop {
                    // "a" "b" is "ab"
                    let following = self.skip_comments(i + 1, None);
                    if self.tokens[following].kind != Kind::Str {
                        break;
                    }
                    i = following;
                    pieces.push((self.tokens[i].start, self.tokens[i].end));
                }
                if let Some(setting) = setting {
                    setting.strings = Some(pieces);
                }
                i + 1
            }
            Kind::Number | Kind::Env => i + 1,
            Kind::Name if self.is_boolean(i) => i + 1,
            Kind::Punct if self.text[token.start] == b'{' => {
                let mut group = Group { start: token.start, ..Group::default() };
                let end = self.parse_settings(i + 1, &mut group, true)?;
                if let Some(setting) = setting {
                    setting.group = Some(group);
                }
                end
            }
            Kind::Punct if matches!(self.text[token.start], b'[' | b'(') => {
                let closing: &[u8] = if self.text[token.start] == b'[' { b"]" } else { b")" };
                i = self.skip_comments(i + 1, None);
                while !self.is_punct(i, closing) {
                    let element = self.parse_value(i, None)?;
                    i = self.skip_comments(element, None);
                    if self.is_punct(i, b",") {
                        i = self.skip_comments(i + 1, None);
                    } else if !self.is_punct(i, closing) {
                        return Err(self.syntax_error(i));
                    }
                }
                i + 1
            }
            _ => return Err(self.syntax_error(i)),
        };
        self.depth -= 1;
        Ok(next)
    }
}

/// The top-level settings in a config file's text, as a `Group`. Fails where
/// libconfig would reject the file's structure.
pub fn parse_config(text: &[u8]) -> Result<Group, ParseError> {
    let (tokens, left_open) = tokenize(text);
    let open_string = left_open.filter(|t| t.kind == Kind::OpenString).map(|t| t.start);
    let mut parser = Parser { text, tokens, open_string, depth: 0 };
    let mut root = Group { start: 0, end: Some(left_open.map_or(text.len(), |t| t.start)), ..Group::default() };
    parser.parse_settings(0, &mut root, false)?;
    Ok(root)
}

/// The setting at a path such as "pipewire.sink_target" in group, or None.
pub fn find_setting<'a>(group: &'a Group, path: &str) -> Option<&'a Setting> {
    let mut group = group;
    let mut names = path.split('.').peekable();
    while let Some(name) = names.next() {
        let setting = group.get(name.as_bytes())?;
        if names.peek().is_none() {
            return Some(setting);
        }
        group = setting.group.as_ref()?;
    }
    None
}

// ---- reading values -------------------------------------------------------

/// `piece` with "${NAME}" replaced by the variable's value (left alone if it
/// isn't set) and "$${" by a literal "${".
fn expand_env_refs(piece: &[u8], env: &dyn Fn(&[u8]) -> Option<Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::with_capacity(piece.len());
    let mut i = 0;
    while i < piece.len() {
        if piece[i..].starts_with(b"$${") {
            out.extend_from_slice(b"${");
            i += 3;
        } else if let Some(end) = env_ref_end(piece, i) {
            match env(&piece[i + 2..end - 1]) {
                Some(value) => out.extend_from_slice(&value),
                None => out.extend_from_slice(&piece[i..end]),
            }
            i = end;
        } else {
            out.push(piece[i]);
            i += 1;
        }
    }
    out
}

fn hex_digit(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// `piece` with libconfig's escapes undone; any other backslash stays.
fn unescape(piece: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(piece.len());
    let mut i = 0;
    while i < piece.len() {
        if piece[i] == b'\\' {
            let simple = match piece.get(i + 1) {
                Some(b'n') => Some(b'\n'),
                Some(b'r') => Some(b'\r'),
                Some(b't') => Some(b'\t'),
                Some(b'f') => Some(0x0c),
                Some(b'"') => Some(b'"'),
                Some(b'\\') => Some(b'\\'),
                _ => None,
            };
            if let Some(byte) = simple {
                out.push(byte);
                i += 2;
                continue;
            }
            if matches!(piece.get(i + 1), Some(b'x' | b'X')) {
                let high = piece.get(i + 2).copied().and_then(hex_digit);
                let low = piece.get(i + 3).copied().and_then(hex_digit);
                if let (Some(high), Some(low)) = (high, low) {
                    out.push(high << 4 | low);
                    i += 4;
                    continue;
                }
            }
        }
        out.push(piece[i]);
        i += 1;
    }
    out
}

fn process_env(name: &[u8]) -> Option<Vec<u8>> {
    std::env::var_os(std::ffi::OsStr::from_bytes(name)).map(OsString::into_vec)
}

/// The value of a string setting as shairport-sync reads it, or "" if the
/// setting isn't a string (shairport-sync ignores it then), with ${NAME}
/// looked up with `env`.
pub fn string_value_with(text: &[u8], setting: &Setting, env: &dyn Fn(&[u8]) -> Option<Vec<u8>>) -> String {
    let Some(strings) = &setting.strings else {
        return String::new();
    };
    let mut value = Vec::new();
    for &(start, end) in strings {
        // ${NAME} is expanded before libconfig undoes its escapes.
        value.extend(unescape(&expand_env_refs(&text[start + 1..end - 1], env)));
    }
    String::from_utf8_lossy(&value).into_owned()
}

/// As `string_value_with`, with ${NAME} expanded from the app's own
/// environment, the nearest to the service's there is.
pub fn string_value(text: &[u8], setting: &Setting) -> String {
    string_value_with(text, setting, &process_env)
}

/// (end, text) of a comment that follows a setting on its line, or None.
pub fn comment_after(text: &[u8], setting: &Setting) -> Option<(usize, Vec<u8>)> {
    let i = skip_blanks(text, setting.end);
    let rest = &text[i..];
    if rest.starts_with(b"//") || rest.starts_with(b"#") {
        let start = if rest[0] == b'#' { i + 1 } else { i + 2 };
        let mut end = start;
        // Up to the end of the line, which a "\r\n" ends at its "\r".
        while end < text.len() && text[end] != b'\n' && !text[end..].starts_with(b"\r\n") {
            end += 1;
        }
        return Some((end, text[start..end].to_vec()));
    }
    if rest.starts_with(b"/*") {
        let mut end = i + 2;
        loop {
            match text.get(end) {
                None | Some(b'\n') => return None,
                Some(_) if text[end..].starts_with(b"*/") => break,
                Some(_) => end += 1,
            }
        }
        return Some((end + 2, text[i + 2..end].to_vec()));
    }
    None
}

// ---- editing --------------------------------------------------------------

/// value as a libconfig string that shairport-sync reads back unchanged.
pub fn quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    for c in value.chars() {
        match c {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\x0c' => quoted.push_str("\\f"),
            c if c < ' ' => quoted.push_str(&format!("\\x{:02x}", c as u32)),
            c => quoted.push(c),
        }
    }
    format!("\"{}\"", quoted.replace("${", "$${"))
}

/// (begin, end) of the lines holding text[start..end], with the newline, if
/// nothing else is on them; else None.
fn own_lines(text: &[u8], start: usize, end: usize) -> Option<(usize, usize)> {
    let begin = text[..start].iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
    let blanks = skip_blanks(text, end);
    let finish = match &text[blanks..] {
        [] => blanks,
        [b'\n', ..] => blanks + 1,
        [b'\r', b'\n', ..] => blanks + 2,
        _ => return None,
    };
    text[begin..start].iter().all(|&b| is_blank(b)).then_some((begin, finish))
}

fn line_ending(line: &[u8]) -> &'static str {
    if line.ends_with(b"\r\n") {
        "\r\n"
    } else if line.ends_with(b"\n") {
        "\n"
    } else {
        ""
    }
}

/// Whether a comment is a line of its own holding a commented-out <key> setting.
fn is_commented_out(text: &[u8], comment: (usize, usize), key: &str) -> bool {
    let (start, end) = comment;
    let body = &text[start..end];
    let mut i = if body.starts_with(b"//") {
        2
    } else if body.starts_with(b"#") {
        1
    } else {
        return false;
    };
    let skip = |mut i: usize| {
        while matches!(body.get(i), Some(b' ' | b'\t')) {
            i += 1;
        }
        i
    };
    i = skip(i);
    if !body[i..].starts_with(key.as_bytes()) {
        return false;
    }
    i = skip(i + key.len());
    matches!(body.get(i), Some(b'=' | b':')) && own_lines(text, start, end).is_some()
}

/// For a setting that shares its line with others.
fn inline(setting: &str, comment: &str) -> String {
    format!("{} /* {} */", setting, comment.replace("*/", "* /"))
}

fn splice(text: &[u8], begin: usize, new: &str, finish: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + new.len());
    out.extend_from_slice(&text[..begin]);
    out.extend_from_slice(new.as_bytes());
    out.extend_from_slice(&text[finish..]);
    out
}

/// text with the group's <key> setting replaced by `setting` (its
/// `key = value;`), followed by comment, or with setting None, removed.
fn edit_group(text: &[u8], group: &Group, key: &str, setting: Option<&str>, comment: &str, off_line: &str) -> Vec<u8> {
    let commented = group.comments.iter().copied().find(|&c| is_commented_out(text, c, key));
    if let Some(current) = group.get(key.as_bytes()) {
        let (mut start, mut end) = (current.start, current.end);
        if let Some((after, _)) = comment_after(text, current) {
            end = after; // the comment after it goes with it
        }
        if let Some((begin, finish)) = own_lines(text, start, end) {
            let newline = line_ending(&text[begin..finish]);
            let new = match setting {
                Some(setting) => {
                    let mut new = text[begin..start].to_vec();
                    new.extend_from_slice(format!("{setting} // {comment}{newline}").as_bytes());
                    let mut out = text[..begin].to_vec();
                    out.extend_from_slice(&new);
                    out.extend_from_slice(&text[finish..]);
                    return out;
                }
                None if commented.is_some() => String::new(), // it's already there commented out
                None => format!("{}{newline}", off_line.trim_end_matches('\n')),
            };
            return splice(text, begin, &new, finish);
        }
        // It shares its line with other settings, so change only its own text.
        if let Some(setting) = setting {
            return splice(text, start, &inline(setting, comment), end);
        }
        let mut finish = skip_blanks(text, end);
        if matches!(text.get(finish), None | Some(b'\r' | b'\n')) {
            finish = end;
            while start > 0 && is_blank(text[start - 1]) {
                start -= 1;
            }
        }
        return splice(text, start, "", finish);
    }
    let Some(setting) = setting else {
        return text.to_vec();
    };
    if let Some((begin, finish)) = commented.and_then(|(start, end)| own_lines(text, start, end)) {
        let newline = line_ending(&text[begin..finish]);
        return splice(text, begin, &format!("\t{setting} // {comment}{newline}"), finish);
    }
    // A new setting goes at the top of the group: on a line of its own if the
    // "{" ends its line, else straight after the "{".
    let brace_end = group.start + 1;
    if let Some(line_end) = rest_of_line_end(text, brace_end) {
        let newline = line_ending(&text[brace_end..line_end]);
        return splice(text, line_end, &format!("\t{setting} // {comment}{newline}"), line_end);
    }
    splice(text, brace_end, &format!(" {}", inline(setting, comment)), brace_end)
}

/// The offset just past the newline of the line `i` is on, if only blanks and
/// a line comment come before it.
fn rest_of_line_end(text: &[u8], i: usize) -> Option<usize> {
    let mut j = skip_blanks(text, i);
    if text[j..].starts_with(b"//") || text[j..].starts_with(b"#") {
        j = find(text, b"\n", j)?;
    } else if text[j..].starts_with(b"\r\n") {
        j += 1;
    }
    (text.get(j) == Some(&b'\n')).then_some(j + 1)
}

fn line_of(text: &[u8], offset: usize) -> usize {
    text[..offset.min(text.len())].iter().filter(|&&b| b == b'\n').count() + 1
}

/// Whether text parses with pipewire.<key> set to value (with value None, unset).
fn edit_is_right(text: &[u8], key: &str, value: Option<&str>, env: &dyn Fn(&[u8]) -> Option<Vec<u8>>) -> bool {
    let Ok(root) = parse_config(text) else {
        return false;
    };
    let setting = find_setting(&root, &format!("pipewire.{key}"));
    match (value, setting) {
        (None, setting) => setting.is_none(),
        (Some(value), Some(setting)) => string_value_with(text, setting, env) == value,
        (Some(_), None) => false,
    }
}

/// `text` with pipewire.<key> set to the string value, followed by comment.
/// With value None, it is unset instead, leaving off_line (the setting
/// commented out) unless it is already commented out somewhere in the group.
/// None if nothing needs to change. `file` names the file in errors: an error
/// if it doesn't parse or can't be changed safely.
pub fn edit_pipewire_setting(
    text: &[u8],
    key: &str,
    value: Option<&str>,
    comment: &str,
    off_line: &str,
    file: &Path,
    env: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>, ConfigError> {
    let file = file.display();
    let root = parse_config(text).map_err(|err| {
        ConfigError::Unsafe(format!("{file}, line {}: {}", line_of(text, err.offset), err.message))
    })?;
    let setting = value.map(|value| format!("{key} = {};", quote(value)));
    let new_text = match root.get(b"pipewire") {
        None => {
            let Some(setting) = setting else {
                return Ok(None);
            };
            // The group goes at the end of the file, before a string or comment
            // left open there, which would swallow it.
            let crlf = matches!(find(text, b"\n", 0), Some(first) if first > 0 && text[first - 1] == b'\r');
            let newline = if crlf { "\r\n" } else { "\n" };
            let lines = ["", "pipewire =", "{", &format!("\t{setting} // {comment}"), "};", ""];
            let end = root.end.unwrap_or(text.len());
            splice(text, end, &lines.join(newline), end)
        }
        Some(pipewire) => match &pipewire.group {
            None => {
                let line = line_of(text, pipewire.start);
                return Err(ConfigError::Unsafe(format!("{file}, line {line}: pipewire isn't a group")));
            }
            Some(group) => edit_group(text, group, key, setting.as_deref(), comment, off_line),
        },
    };
    if new_text == text {
        return Ok(None);
    }
    // A last check, in case the edit misread the layout.
    if !edit_is_right(&new_text, key, value, env) {
        return Err(ConfigError::Unsafe(format!("{file}: pipewire.{key} can't be changed safely in this file")));
    }
    Ok(Some(new_text))
}

/// `text` with pipewire.sink_target set to the output `name`, labelled
/// `label` in a comment, or with an empty name, unset (follow the default).
/// See `edit_pipewire_setting`.
pub fn edit_sink_target(
    text: &[u8],
    name: &str,
    label: &str,
    file: &Path,
    env: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>, ConfigError> {
    if name.is_empty() {
        return edit_pipewire_setting(text, "sink_target", None, "", SINK_TARGET_DEFAULT_LINE, file, env);
    }
    // The label goes in a comment, so keep it to one line of printable text.
    let comment = format!("{LABEL_MARKER}{}", clean_label(label));
    edit_pipewire_setting(text, "sink_target", Some(name), &comment, SINK_TARGET_DEFAULT_LINE, file, env)
}

/// `text` with pipewire.mixer_type set to "sink", or unset. See
/// `edit_pipewire_setting`.
pub fn edit_device_volume(
    text: &[u8],
    enabled: bool,
    file: &Path,
    env: &dyn Fn(&[u8]) -> Option<Vec<u8>>,
) -> Result<Option<Vec<u8>>, ConfigError> {
    let value = enabled.then_some("sink");
    edit_pipewire_setting(text, "mixer_type", value, MIXER_TYPE_ON_COMMENT, MIXER_TYPE_OFF_LINE, file, env)
}

/// Whether Python's str.isspace() holds for c, which str.split() and
/// str.strip() go by.
fn is_py_space(c: char) -> bool {
    c.is_whitespace() || ('\x1c'..='\x1f').contains(&c)
}

/// A label kept to one line of printable text, as it goes in a comment.
pub fn clean_label(label: &str) -> String {
    let spaced: String = label
        .chars()
        .map(|c| if matches!(c, '\0'..='\x1f' | '\x7f'..='\u{9f}') { ' ' } else { c })
        .collect();
    spaced.split(is_py_space).filter(|word| !word.is_empty()).collect::<Vec<_>>().join(" ")
}

/// (node.name, label) from a found pipewire.sink_target; ("", "") means
/// follow the default.
pub fn sink_target_of(text: &[u8], setting: &Setting, env: &dyn Fn(&[u8]) -> Option<Vec<u8>>) -> (String, String) {
    let name = string_value_with(text, setting, env);
    if name.is_empty() {
        return (String::new(), String::new());
    }
    let label = comment_after(text, setting)
        .and_then(|(_, comment)| {
            find(&comment, LABEL_MARKER.as_bytes(), 0)
                .map(|at| String::from_utf8_lossy(&comment[at + LABEL_MARKER.len()..]).into_owned())
        })
        .unwrap_or_default();
    (name, label.trim_matches(is_py_space).to_string())
}

// ---- the files ------------------------------------------------------------

static TMP_COUNTER: AtomicU32 = AtomicU32::new(0);

/// A new file next to `path`, named after it, that only the user can read.
pub fn create_temp_file(path: &Path) -> io::Result<(fs::File, PathBuf)> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path.file_name().unwrap_or_default();
    loop {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let unique = format!(
            ".{:x}{:x}{:x}.tmp",
            std::process::id(),
            nanos,
            TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let mut tmp_name = OsString::from(".");
        tmp_name.push(name);
        tmp_name.push(unique);
        let tmp = dir.join(tmp_name);
        match fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp) {
            Ok(file) => return Ok((file, tmp)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
}

fn is_writable(path: &Path) -> bool {
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: c_path is a valid NUL-terminated string for the length of the call.
    unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 }
}

/// Atomically replace path with text, keeping its permissions and, where
/// allowed, its owner.
fn save_config(path: &Path, text: &[u8]) -> io::Result<()> {
    if !is_writable(path) {
        return Err(io::Error::from_raw_os_error(libc::EACCES));
    }
    let info = fs::metadata(path)?;
    let (mut file, tmp) = create_temp_file(path)?;
    let written = (|| {
        file.write_all(text)?;
        file.flush()?;
        file.set_permissions(fs::Permissions::from_mode(info.mode() & 0o7777))?;
        let _ = std::os::unix::fs::fchown(&file, Some(info.uid()), Some(info.gid()));
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// The user's home directory, as Python's Path.home() finds it.
pub fn home_dir() -> PathBuf {
    // As posixpath.expanduser does, an empty home is the root directory.
    fn or_root(home: &std::ffi::OsStr) -> PathBuf {
        if home.is_empty() {
            PathBuf::from("/")
        } else {
            PathBuf::from(home)
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return or_root(&home);
    }
    // SAFETY: getpwuid returns NULL or a pointer to a static passwd entry
    // whose pw_dir, when not NULL, is a NUL-terminated string; it is copied
    // before anything else can call getpwuid.
    unsafe {
        let entry = libc::getpwuid(libc::getuid());
        if !entry.is_null() && !(*entry).pw_dir.is_null() {
            let dir = std::ffi::CStr::from_ptr((*entry).pw_dir);
            return or_root(std::ffi::OsStr::from_bytes(dir.to_bytes()));
        }
    }
    PathBuf::from("/")
}

/// The config files the app reads and the one it writes.
#[derive(Debug, Clone)]
pub struct Config {
    /// The user's config, which shairport-sync reads and the app changes.
    pub file: PathBuf,
    /// A new `file` starts as a copy of the packages' desktop defaults or,
    /// where those aren't installed (a from-source install), of the
    /// system-wide config.
    pub defaults_file: PathBuf,
    pub system_file: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            file: home_dir().join(".config/shairport-sync/shairport-sync.conf"),
            defaults_file: PathBuf::from("/etc/shairport-sync-desktop.conf"),
            system_file: PathBuf::from("/etc/shairport-sync.conf"),
        }
    }
}

impl Config {
    /// The file a new config file is copied from.
    fn seed_file(&self) -> &Path {
        if self.defaults_file.exists() {
            &self.defaults_file
        } else {
            &self.system_file
        }
    }

    /// The packaged user service seeds the per-user file from the desktop
    /// defaults on first start; do the same here so it can be edited before
    /// that.
    pub fn ensure_user_config(&self) -> Result<(), ConfigError> {
        let seed = self.seed_file();
        if !self.file.exists() && seed.exists() {
            if let Some(dir) = self.file.parent() {
                fs::create_dir_all(dir).map_err(io_error(dir))?;
            }
            let text = fs::read(seed).map_err(io_error(seed))?;
            fs::write(&self.file, text).map_err(io_error(&self.file))?;
        }
        Ok(())
    }

    /// The text of the config shairport-sync reads when it starts: the user's
    /// or, if that can't be read (before it exists), the file it will be
    /// copied from. None if there is neither.
    fn effective_text(&self) -> Option<Vec<u8>> {
        [self.file.as_path(), self.seed_file()].into_iter().find_map(|file| fs::read(file).ok())
    }

    /// (text, setting) of a setting such as "pipewire.sink_target". None if
    /// it isn't set, or if there is no file or it doesn't parse
    /// (shairport-sync won't start then, so nothing is in use).
    fn lookup(&self, path: &str) -> Option<(Vec<u8>, Setting)> {
        let text = self.effective_text()?;
        let setting = find_setting(&parse_config(&text).ok()?, path)?.clone();
        Some((text, setting))
    }

    /// The value of a string setting as shairport-sync reads it, or "" if it
    /// isn't set or isn't a string.
    fn string_setting(&self, path: &str) -> String {
        self.lookup(path).map(|(text, setting)| string_value(&text, &setting)).unwrap_or_default()
    }

    /// Whether the config shairport-sync reads when it starts parses; it
    /// won't start with one that doesn't.
    pub fn parses(&self) -> bool {
        self.effective_text().is_some_and(|text| parse_config(&text).is_ok())
    }

    /// (node.name, label) from pipewire.sink_target; ("", "") means follow
    /// the default.
    pub fn read_sink_target(&self) -> (String, String) {
        match self.lookup("pipewire.sink_target") {
            Some((text, setting)) => sink_target_of(&text, &setting, &process_env),
            None => (String::new(), String::new()),
        }
    }

    pub fn write_sink_target(&self, name: &str, label: &str) -> Result<(), ConfigError> {
        self.edit(|text| edit_sink_target(text, name, label, &self.file, &process_env))
    }

    pub fn read_device_volume(&self) -> bool {
        // Compared as shairport-sync does: ignoring case, but not blanks.
        self.string_setting("pipewire.mixer_type").eq_ignore_ascii_case("sink")
    }

    pub fn write_device_volume(&self, enabled: bool) -> Result<(), ConfigError> {
        self.edit(|text| edit_device_volume(text, enabled, &self.file, &process_env))
    }

    pub fn uses_pipewire_backend(&self) -> bool {
        // shairport-sync matches the name ignoring case. With none set it uses
        // its first backend, which in the packaged builds is ALSA.
        self.string_setting("general.output_backend").eq_ignore_ascii_case("pipewire")
    }

    /// Change the user's config with one of the edit_* functions.
    fn edit(&self, edit: impl FnOnce(&[u8]) -> Result<Option<Vec<u8>>, ConfigError>) -> Result<(), ConfigError> {
        self.ensure_user_config()?;
        // A symlinked config is edited where it lives.
        let path = fs::canonicalize(&self.file).unwrap_or_else(|_| self.file.clone());
        let text = fs::read(&path).map_err(io_error(&path))?;
        match edit(&text)? {
            Some(new_text) => save_config(&path, &new_text).map_err(io_error(&path)),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    //! The cases in tests/config_cases.json were made by the Python app this
    //! one was ported from: what it found in each config text, and what each
    //! of its edits made of it.

    use super::*;
    use serde_json::Value;
    use std::collections::HashMap;

    fn unhex(hex: &str) -> Vec<u8> {
        (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
    }

    fn pairs(ranges: &[(usize, usize)]) -> Value {
        ranges.iter().map(|&(start, end)| serde_json::json!([start, end])).collect()
    }

    fn dump_group(group: &Group) -> Value {
        serde_json::json!({
            "start": group.start,
            "end": group.end,
            "settings": group.settings.iter().map(|(name, setting)| serde_json::json!([
                String::from_utf8_lossy(name), dump_setting(setting)])).collect::<Vec<_>>(),
            "comments": pairs(&group.comments),
        })
    }

    fn dump_setting(setting: &Setting) -> Value {
        serde_json::json!({
            "start": setting.start,
            "end": setting.end,
            "strings": setting.strings.as_deref().map(pairs),
            "group": setting.group.as_ref().map(dump_group),
        })
    }

    fn dump_edit(text: &[u8], result: Result<Option<Vec<u8>>, ConfigError>) -> Value {
        match result {
            Err(err) => serde_json::json!({ "err": err.to_string() }),
            Ok(None) => serde_json::json!({ "same": true }),
            Ok(Some(new)) => {
                let shorter = text.len().min(new.len());
                let prefix = (0..shorter).find(|&i| text[i] != new[i]).unwrap_or(shorter);
                let suffix = (0..shorter - prefix)
                    .find(|&i| text[text.len() - 1 - i] != new[new.len() - 1 - i])
                    .unwrap_or(shorter - prefix);
                let middle: String = new[prefix..new.len() - suffix].iter().map(|b| format!("{b:02x}")).collect();
                serde_json::json!({ "new": [prefix, suffix, middle] })
            }
        }
    }

    #[test]
    fn matches_the_python_app() {
        let path = std::env::var_os("CONFIG_CASES")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/config_cases.json"));
        let golden: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let vars: HashMap<Vec<u8>, Vec<u8>> = golden["env"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, value)| (name.clone().into_bytes(), value.as_str().unwrap().as_bytes().to_vec()))
            .collect();
        let env = |name: &[u8]| vars.get(name).cloned();
        let file = Path::new("CONF");
        let cases = golden["cases"].as_array().unwrap();
        assert!(!cases.is_empty());

        for (number, case) in cases.iter().enumerate() {
            let text = unhex(case["text"].as_str().unwrap());
            let shown = String::from_utf8_lossy(&text);
            let parsed = parse_config(&text);
            let got = match &parsed {
                Ok(root) => serde_json::json!({ "ok": dump_group(root) }),
                Err(err) => serde_json::json!({ "err": [err.offset, err.message] }),
            };
            assert_eq!(got, case["parse"], "case {number}: parse of {shown:?}");

            let string_of = |path: &str| {
                let root = parsed.as_ref().ok()?;
                Some(string_value_with(&text, find_setting(root, path)?, &env))
            };
            if let Ok(root) = &parsed {
                for (path, expected) in case["reads"].as_object().unwrap() {
                    let got = find_setting(root, path).map(|setting| {
                        let comment = comment_after(&text, setting).map(|(end, comment)| {
                            serde_json::json!([end, comment.iter().map(|b| format!("{b:02x}")).collect::<String>()])
                        });
                        serde_json::json!({ "value": string_value_with(&text, setting, &env), "comment": comment })
                    });
                    assert_eq!(serde_json::json!(got), *expected, "case {number}: {path} in {shown:?}");
                }
            }
            let sink_target = parsed
                .as_ref()
                .ok()
                .and_then(|root| find_setting(root, "pipewire.sink_target"))
                .map(|setting| sink_target_of(&text, setting, &env))
                .unwrap_or_default();
            assert_eq!(serde_json::json!([sink_target.0, sink_target.1]), case["sink_target"], "case {number}: {shown:?}");
            let device_volume = string_of("pipewire.mixer_type").unwrap_or_default().eq_ignore_ascii_case("sink");
            assert_eq!(serde_json::json!(device_volume), case["device_volume"], "case {number}: {shown:?}");
            let backend = string_of("general.output_backend").unwrap_or_default().eq_ignore_ascii_case("pipewire");
            assert_eq!(serde_json::json!(backend), case["pipewire_backend"], "case {number}: {shown:?}");
            assert_eq!(serde_json::json!(parsed.is_ok()), case["parses"], "case {number}: {shown:?}");

            for op in case["ops"].as_array().unwrap() {
                let result = match op["op"].as_str().unwrap() {
                    "sink" => {
                        edit_sink_target(&text, op["name"].as_str().unwrap(), op["label"].as_str().unwrap(), file, &env)
                    }
                    "volume" => edit_device_volume(&text, op["on"].as_bool().unwrap(), file, &env),
                    other => panic!("unknown op {other}"),
                };
                assert_eq!(dump_edit(&text, result), op["result"], "case {number}: {op} on {shown:?}");
            }
        }
    }

    #[test]
    fn edits_the_file_in_place() {
        let dir = std::env::temp_dir().join(format!("shairport-sync-gui-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("real")).unwrap();
        let config = Config {
            file: dir.join("user/shairport-sync.conf"),
            defaults_file: dir.join("defaults.conf"),
            system_file: dir.join("system.conf"),
        };
        assert!(!config.parses());
        assert_eq!(config.read_sink_target(), (String::new(), String::new()));
        fs::write(&config.system_file, "general = { output_backend = \"pipewire\"; };\npipewire =\n{\n};\n").unwrap();
        assert!(config.parses() && config.uses_pipewire_backend() && !config.read_device_volume());

        // The first write seeds the user's file, here through a symlink.
        fs::create_dir_all(dir.join("user")).unwrap();
        let real = dir.join("real/shairport-sync.conf");
        fs::write(&real, fs::read(&config.system_file).unwrap()).unwrap();
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
        std::os::unix::fs::symlink(&real, &config.file).unwrap();
        config.write_sink_target("alsa_output.usb", "USB\tAudio").unwrap();
        config.write_device_volume(true).unwrap();
        assert_eq!(config.read_sink_target(), ("alsa_output.usb".to_string(), "USB Audio".to_string()));
        assert!(config.read_device_volume());
        assert!(fs::symlink_metadata(&config.file).unwrap().file_type().is_symlink());
        assert_eq!(fs::metadata(&real).unwrap().mode() & 0o7777, 0o640);
        assert_eq!(fs::read_dir(dir.join("real")).unwrap().count(), 1, "no temporary file is left");

        config.write_sink_target("", "").unwrap();
        config.write_device_volume(false).unwrap();
        assert_eq!(config.read_sink_target(), (String::new(), String::new()));
        assert!(!config.read_device_volume());

        // A read-only file is left alone, as is one that doesn't parse.
        fs::set_permissions(&real, fs::Permissions::from_mode(0o440)).unwrap();
        if is_writable(&real) {
            // (root can write anything)
        } else {
            assert!(matches!(config.write_device_volume(true), Err(ConfigError::Io { .. })));
        }
        fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
        fs::write(&real, "pipewire = {\n").unwrap();
        let err = config.write_device_volume(true).unwrap_err().to_string();
        assert_eq!(err, format!("{}, line 2: unexpected end of file", config.file.display()));
        assert_eq!(fs::read(&real).unwrap(), b"pipewire = {\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
