//! Line-based text edits for packages (`<text>` commands).

use super::package::{Condition, TextCmd};

/// Delphi-style mask (as used by OpenIV): `*` any run, `?` any character,
/// `[abc]` / `[a-z]` character sets; case-insensitive.
fn mask_matches(mask: &str, text: &str) -> bool {
    fn rec(m: &[char], t: &[char]) -> bool {
        match m.first() {
            None => t.is_empty(),
            Some('*') => (0..=t.len()).any(|i| rec(&m[1..], &t[i..])),
            Some('?') => !t.is_empty() && rec(&m[1..], &t[1..]),
            Some('[') => {
                let Some(close) = m.iter().position(|&c| c == ']') else {
                    return !t.is_empty() && t[0] == '[' && rec(&m[1..], &t[1..]);
                };
                let Some(&c) = t.first() else { return false };
                let set = &m[1..close];
                let mut hit = false;
                let mut i = 0;
                while i < set.len() {
                    if i + 2 < set.len() && set[i + 1] == '-' {
                        hit |= (set[i]..=set[i + 2]).contains(&c);
                        i += 3;
                    } else {
                        hit |= set[i] == c;
                        i += 1;
                    }
                }
                hit && rec(&m[close + 1..], &t[1..])
            }
            Some(&c) => t.first() == Some(&c) && rec(&m[1..], &t[1..]),
        }
    }
    let m: Vec<char> = mask.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    rec(&m, &t)
}

fn matches(line: &str, pattern: &str, condition: Condition) -> bool {
    match condition {
        Condition::Equal => line == pattern,
        Condition::StartWith => line.starts_with(pattern),
        Condition::Mask => mask_matches(pattern, line),
    }
}

/// Applies text commands. Returns the new text and notes about commands that matched nothing.
pub fn apply(text: &str, commands: &[TextCmd]) -> (String, Vec<String>) {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let trailing_newline = text.ends_with('\n');
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let mut notes = Vec::new();
    for cmd in commands {
        match cmd {
            TextCmd::Add(line) => lines.push(line.clone()),
            TextCmd::Insert { before, line, condition, text } => {
                match lines.iter().position(|l| matches(l, line, *condition)) {
                    Some(i) => lines.insert(if *before { i } else { i + 1 }, text.clone()),
                    None => notes.push(format!("insert: no line matches \"{line}\"")),
                }
            }
            TextCmd::Replace { line, condition, text } => {
                let mut hit = false;
                for l in lines.iter_mut().filter(|l| matches(l, line, *condition)) {
                    *l = text.clone();
                    hit = true;
                }
                if !hit {
                    notes.push(format!("replace: no line matches \"{line}\""));
                }
            }
            TextCmd::Delete { line, condition } => {
                let before = lines.len();
                lines.retain(|l| !matches(l, line, *condition));
                if lines.len() == before {
                    notes.push(format!("delete: no line matches \"{line}\""));
                }
            }
        }
    }
    let mut out = lines.join(nl);
    if trailing_newline || text.is_empty() {
        out.push_str(nl);
    }
    (out, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_example() {
        // The text commands of the official 2.1/2.2 example package.
        let input = "Line 1\nLine 2\n# THIS LINE\nLine 3\nLine 4\nLine 5\n#####\n";
        let cmds = [
            TextCmd::Add("This line is added".into()),
            TextCmd::Delete { line: "Line 3".into(), condition: Condition::Equal },
            TextCmd::Replace {
                line: "Line 4".into(),
                condition: Condition::StartWith,
                text: "THIS IS NEW LINE".into(),
            },
            TextCmd::Insert {
                before: false,
                line: "Line 5".into(),
                condition: Condition::Equal,
                text: "This is last line".into(),
            },
            TextCmd::Insert {
                before: true,
                line: "Line 1".into(),
                condition: Condition::Equal,
                text: "This is first line".into(),
            },
            TextCmd::Delete { line: "#*".into(), condition: Condition::Mask },
        ];
        let (out, notes) = apply(input, &cmds);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(
            out,
            "This is first line\nLine 1\nLine 2\nTHIS IS NEW LINE\nLine 5\nThis is last line\nThis line is added\n"
        );
    }

    #[test]
    fn masks() {
        assert!(mask_matches("*line*", "# THIS LINE"));
        assert!(mask_matches("l?ne [0-9]", "Line 7"));
        assert!(!mask_matches("#*", "Line"));
        let (out, notes) = apply(
            "a\r\nb\r\n",
            &[TextCmd::Replace { line: "x".into(), condition: Condition::Equal, text: "y".into() }],
        );
        assert_eq!(out, "a\r\nb\r\n");
        assert_eq!(notes.len(), 1);
    }
}
