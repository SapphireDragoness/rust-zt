//! The game's INI dialect (`.ai`, `.cfg`, `.scn`, `.lyt`, `.ani`, `zoo.ini`, …).
//!
//! * Section and key names are case-insensitive; order is preserved.
//! * Keys repeat (`animation = N`, `cAdultAdmission = 49`, …), so every
//!   occurrence is kept.
//! * Lines without `=` are kept as bare entries (the `[Member]` lists of
//!   `.ai` files).
//! * Only whole-line comments (`;`, `#`, `//`) are stripped. Values may legally
//!   contain `;` (`path=./updates;./loc;.`). Numeric getters ignore trailing
//!   junk such as `-20;` or tabs.

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ini {
    pub sections: Vec<Section>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Section {
    pub name: String,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub key: String,
    /// `None` for bare lines without `=`.
    pub value: Option<String>,
}

impl Ini {
    pub fn parse(text: &str) -> Ini {
        let mut ini = Ini::default();
        // Entries before the first section header go into a nameless section.
        let mut cur = Section::default();
        let mut have_cur = false;
        for raw in text.lines() {
            let line = raw.trim_matches(|c: char| c.is_whitespace() || c == '\u{1a}' || c == '\0');
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') || line.starts_with("//")
            {
                continue;
            }
            if line.starts_with('[') {
                if let Some(end) = line.find(']') {
                    if have_cur || !cur.entries.is_empty() {
                        ini.sections.push(std::mem::take(&mut cur));
                    }
                    cur.name = line[1..end].trim().to_string();
                    have_cur = true;
                    continue;
                }
            }
            match line.split_once('=') {
                Some((k, v)) => cur.entries.push(Entry {
                    key: k.trim().to_string(),
                    value: Some(v.trim().to_string()),
                }),
                None => cur.entries.push(Entry { key: line.to_string(), value: None }),
            }
        }
        if have_cur || !cur.entries.is_empty() {
            ini.sections.push(cur);
        }
        ini
    }

    /// First section with this name.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    /// All sections with this name, in order.
    pub fn sections_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Section> + 'a {
        self.sections.iter().filter(move |s| s.name.eq_ignore_ascii_case(name))
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.sections
            .iter()
            .filter(|s| s.name.eq_ignore_ascii_case(section))
            .find_map(|s| s.get(key))
    }

    pub fn get_int(&self, section: &str, key: &str) -> Option<i64> {
        self.get(section, key).and_then(parse_int)
    }

    pub fn get_float(&self, section: &str, key: &str) -> Option<f64> {
        self.get(section, key).and_then(parse_float)
    }
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|e| e.key.eq_ignore_ascii_case(key) && e.value.is_some())
            .and_then(|e| e.value.as_deref())
    }

    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.entries
            .iter()
            .filter(move |e| e.key.eq_ignore_ascii_case(key))
            .filter_map(|e| e.value.as_deref())
    }

    pub fn get_int(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(parse_int)
    }

    pub fn get_float(&self, key: &str) -> Option<f64> {
        self.get(key).and_then(parse_float)
    }

    /// Bare (`=`-less) lines, e.g. the class names of a `[Member]` section.
    pub fn bare(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().filter(|e| e.value.is_none()).map(|e| e.key.as_str())
    }
}

fn numeric_prefix(s: &str, allow_float: bool) -> &str {
    let s = s.trim_start();
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        i += 1;
    }
    let mut seen_dot = false;
    let mut seen_exp = false;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() {
            i += 1;
        } else if allow_float && c == b'.' && !seen_dot && !seen_exp {
            seen_dot = true;
            i += 1;
        } else if allow_float
            && (c == b'e' || c == b'E')
            && !seen_exp
            && i > 0
            && b.get(i + 1).is_some_and(|n| n.is_ascii_digit() || *n == b'-' || *n == b'+')
        {
            seen_exp = true;
            i += 2;
        } else {
            break;
        }
    }
    &s[..i]
}

/// Parse a leading integer, ignoring trailing junk (`-20;` → -20, `0x1F` → 31).
pub fn parse_int(s: &str) -> Option<i64> {
    let t = s.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        let end = h.find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(h.len());
        return i64::from_str_radix(&h[..end], 16).ok();
    }
    let p = numeric_prefix(t, false);
    if p.is_empty() {
        return None;
    }
    p.parse().ok().or_else(|| {
        // "3.5" in an integer slot: truncate like atoi would.
        parse_float(t).map(|f| f as i64)
    })
}

/// Parse a leading float, ignoring trailing junk.
pub fn parse_float(s: &str) -> Option<f64> {
    let p = numeric_prefix(s, true);
    if p.is_empty() || p == "-" || p == "+" {
        return None;
    }
    p.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dialect() {
        let ini = Ini::parse(
            "[Global]\nClass = ztunit\n\n[Member]\nentity\nunit\n\n[Characteristics/Integers]\n\
             cCaptivityCheck = 20\t\t\t\ncDeathChange = -20;\n; comment\ncPct = 0.3\n\
             [resource]\npath=./updates;./loc;.\n[a]\nk=1\nk=2\n",
        );
        assert_eq!(ini.get("global", "class"), Some("ztunit"));
        assert_eq!(ini.section("member").unwrap().bare().collect::<Vec<_>>(), ["entity", "unit"]);
        assert_eq!(ini.get_int("Characteristics/Integers", "cCaptivityCheck"), Some(20));
        assert_eq!(ini.get_int("Characteristics/Integers", "cDeathChange"), Some(-20));
        assert_eq!(ini.get_float("Characteristics/Integers", "cpct"), Some(0.3));
        assert_eq!(ini.get("resource", "path"), Some("./updates;./loc;."));
        assert_eq!(ini.section("a").unwrap().get_all("k").collect::<Vec<_>>(), ["1", "2"]);
    }
}
