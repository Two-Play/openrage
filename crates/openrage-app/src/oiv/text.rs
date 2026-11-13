//! Line-based text edits for packages (`<text>` commands).

use super::package::{Condition, TextCmd};

/// One element of a mask.
enum Token {
    /// `*`: any run of characters.
    Any,
    /// `?`: any one character.
    One,
    /// `[a-z0]`: one character from the ranges.
    Set(Vec<(char, char)>),
    Char(char),
}

fn tokens(mask: &[char]) -> Vec<Token> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < mask.len() {
        let close = (mask[i] == '[').then(|| mask[i..].iter().position(|&c| c == ']').map(|p| i + p)).flatten();
        match (mask[i], close) {
            ('*', _) => out.push(Token::Any),
            ('?', _) => out.push(Token::One),
            ('[', Some(close)) => {
                let set = &mask[i + 1..close];
                let mut ranges = Vec::new();
                let mut j = 0;
                while j < set.len() {
                    if j + 2 < set.len() && set[j + 1] == '-' {
                        ranges.push((set[j], set[j + 2]));
                        j += 3;
                    } else {
                        ranges.push((set[j], set[j]));
                        j += 1;
                    }
                }
                out.push(Token::Set(ranges));
                i = close;
            }
            (c, _) => out.push(Token::Char(c)),
        }
        i += 1;
    }
    out
}

/// Delphi-style mask (as used by OpenIV): `*` any run, `?` any character,
/// `[abc]` / `[a-z]` character sets; case-insensitive.
///
/// Iterative, backtracking only to the last `*`: linear stack and at most
/// mask × text steps, whatever a package puts in its masks.
fn mask_matches(mask: &str, text: &str) -> bool {
    let m: Vec<char> = mask.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let m = tokens(&m);
    let one = |token: &Token, c: char| match token {
        Token::Any => false,
        Token::One => true,
        Token::Set(ranges) => ranges.iter().any(|&(a, b)| (a..=b).contains(&c)),
        Token::Char(x) => *x == c,
    };
    let (mut mi, mut ti) = (0, 0);
    // The last `*` seen and the text position it currently stands for.
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match m.get(mi) {
            Some(Token::Any) => {
                star = Some((mi, ti));
                mi += 1;
            }
            Some(token) if one(token, t[ti]) => {
                mi += 1;
                ti += 1;
            }
            _ => match star {
                // Let the last `*` take one more character and try again.
                Some((s, at)) => {
                    star = Some((s, at + 1));
                    mi = s + 1;
                    ti = at + 1;
                }
                None => return false,
            },
        }
    }
    m[mi..].iter().all(|token| matches!(token, Token::Any))
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
        assert!(mask_matches("a*b*c", "aXXbYYc"));
        assert!(!mask_matches("a*b*c", "aXXbYY"));
        assert!(mask_matches("*", ""));
        assert!(mask_matches("[ab]x", "bx") && !mask_matches("[ab]x", "cx"));
        assert!(mask_matches("[x", "[x"), "an unclosed set is a literal [");
        assert!(mask_matches("**a", "a"));
    }

    #[test]
    fn hostile_masks_finish() {
        // Recursion used to overflow the stack on a long mask and backtrack
        // for a very long time on many stars.
        let long = "?".repeat(1_000_000);
        assert!(!mask_matches(&long, "short"));
        let stars = "*a".repeat(200) + "b";
        assert!(!mask_matches(&stars, &"a".repeat(5_000)));
        let (out, notes) = apply(
            "a\r\nb\r\n",
            &[TextCmd::Replace { line: "x".into(), condition: Condition::Equal, text: "y".into() }],
        );
        assert_eq!(out, "a\r\nb\r\n");
        assert_eq!(notes.len(), 1);
    }
}
