/// Cuts method bodies out of decompiled source by matching braces. Vineflower output is regular
/// enough that a declaration is a line that names the method, opens a block and is not a
/// statement. Blocks come back in source order, one per overload, with the 1 based line they
/// start on.
pub fn method_blocks(body: &str, method: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = body.lines().collect();
    let needle = format!("{method}(");
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !declares(lines[i], &needle) {
            i += 1;
            continue;
        }

        let mut start = i;
        while start > 0 && lines[start - 1].trim_start().starts_with('@') {
            start -= 1;
        }

        let mut depth = 0i32;
        let mut opened = false;
        let mut end = i;
        for (j, line) in lines.iter().enumerate().skip(i) {
            for ch in code_chars(line) {
                match ch {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            end = j;
            // An abstract or interface method ends at its semicolon.
            if (opened && depth <= 0) || (!opened && line.trim_end().ends_with(';')) {
                break;
            }
        }
        blocks.push((start + 1, lines[start..=end].join("\n")));
        i = end + 1;
    }
    blocks
}

fn declares(line: &str, needle: &str) -> bool {
    let trimmed = line.trim_start();
    let Some(at) = trimmed.find(needle) else { return false };
    if at == 0 {
        // `Name(` at the very start is a constructor only if something like a modifier is
        // missing, which Vineflower does for package private ones.
        return trimmed.ends_with('{');
    }
    let before = &trimmed[..at];
    let boundary = before.ends_with(' ');
    boundary
        && !before.contains('=')
        && !before.contains('(')
        && !before.trim_end().ends_with("new")
        && !trimmed.starts_with("return ")
        && !trimmed.starts_with("throw ")
        && !trimmed.starts_with("//")
}

/// The characters of a line that are code, with string and char literals blanked out so that
/// a brace inside `"{"` does not count.
fn code_chars(line: &str) -> impl Iterator<Item = char> + '_ {
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;
    line.chars().map(move |ch| {
        if escaped {
            escaped = false;
            return ' ';
        }
        match ch {
            '\\' if in_string || in_char => {
                escaped = true;
                ' '
            }
            '"' if !in_char => {
                in_string = !in_string;
                ' '
            }
            '\'' if !in_string => {
                in_char = !in_char;
                ' '
            }
            _ if in_string || in_char => ' ',
            other => other,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::method_blocks;

    const SOURCE: &str = r#"
public class Lamp extends Light {
   private boolean dim(final Room room, final Wick wick) {
      List<Wick> wicks = room.find(Wick.class, new Area(wick), Wick::isLit);
      if (wicks.isEmpty()) {
         return false;
      } else {
         wicks.get(0).snuff();
         return true;
      }
   }

   @Override
   protected Result use(Switch state) {
      String odd = "}{";
      return this.dim(null, null) ? Result.DONE : Result.PASS;
   }

   protected abstract void use(int other);
}
"#;

    #[test]
    fn finds_the_declaration_not_the_call() {
        let blocks = method_blocks(SOURCE, "dim");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, 3);
        assert!(blocks[0].1.trim_start().starts_with("private boolean dim"));
        assert!(blocks[0].1.trim_end().ends_with('}'));
    }

    #[test]
    fn keeps_annotations_and_ignores_braces_in_strings() {
        let blocks = method_blocks(SOURCE, "use");
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].1.trim_start().starts_with("@Override"));
        assert!(blocks[0].1.contains("Result.PASS"));
        assert_eq!(blocks[1].1.trim(), "protected abstract void use(int other);");
    }
}
