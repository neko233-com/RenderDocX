//! RenderDoc-style expansion of captured shader editor files.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SourceFile {
    pub name: String,
    pub contents: String,
}

fn pragma_once(line: &str) -> Option<usize> {
    let start = line.trim_start_matches([' ', '\t']);
    let after_hash = start.strip_prefix('#')?.trim_start_matches([' ', '\t']);
    let after_pragma = after_hash.strip_prefix("pragma")?;
    if !after_pragma.starts_with([' ', '\t']) {
        return None;
    }
    let rest = after_pragma.trim_start_matches([' ', '\t']);
    rest.strip_prefix("once")?;
    Some(line.len() - rest.len() + 4)
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn expand(
    source: &str,
    files: &[SourceFile],
    included: &mut HashSet<String>,
    stack: &mut Vec<String>,
) -> Result<String, String> {
    let mut output = String::new();
    for line in source.split_inclusive('\n') {
        if let Some(end) = pragma_once(line) {
            output.push_str("// #pragma once");
            output.push_str(&line[end..]);
            continue;
        }
        let trimmed = line.trim_start_matches([' ', '\t']);
        let Some(rest) = trimmed.strip_prefix("#include") else {
            output.push_str(line);
            continue;
        };
        let rest = rest.trim_start_matches([' ', '\t']);
        let (delimiter, closing) = match rest.chars().next() {
            Some('"') => ('"', '"'),
            Some('<') => ('<', '>'),
            _ => return Err(format!("Invalid #include directive: {}", line.trim_end())),
        };
        let after = &rest[delimiter.len_utf8()..];
        let end = after
            .find(closing)
            .ok_or_else(|| format!("Invalid #include directive: {}", line.trim_end()))?;
        let name = &after[..end];
        let file = files.iter().find(|file| file.name == name).or_else(|| {
            files
                .iter()
                .find(|file| basename(&file.name).to_lowercase() == basename(name).to_lowercase())
        });
        let expanded = if let Some(file) = file {
            if stack.contains(&file.name) {
                format!("// not recursively including {name}\n")
            } else if included.contains(&file.name)
                && file
                    .contents
                    .lines()
                    .any(|line| pragma_once(line).is_some())
            {
                format!("// not re-including {name} (pragma once)\n")
            } else {
                included.insert(file.name.clone());
                stack.push(file.name.clone());
                let nested = expand(&file.contents, files, included, stack)?;
                stack.pop();
                nested
            }
        } else {
            format!("// Can't find file {name}\n")
        };
        output.push_str(&line[..line.len() - trimmed.len()]);
        output.push_str("\n\n");
        output.push_str(&expanded);
        output.push_str("\n\n");
    }
    for file in files.iter().filter(|file| file.name == "@cmdline") {
        output = format!("{}\n\n{}", file.contents, output);
    }
    Ok(output)
}

pub fn assemble(files: &[SourceFile]) -> Result<String, String> {
    let first = files.first().ok_or("No shader source files")?;
    let mut included = HashSet::from([first.name.clone()]);
    expand(
        &first.contents,
        files,
        &mut included,
        &mut vec![first.name.clone()],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_includes_once_cycles_missing_and_unicode() {
        let files = vec![
            SourceFile {
                name: "main.hlsl".into(),
                contents:
                    "#include \"inc.hlsl\"\n#include <INC.HLSL>\n#include \"missing\"\nfloat x;\n"
                        .into(),
            },
            SourceFile {
                name: "dir/inc.hlsl".into(),
                contents: "#pragma once\n// 中文\n#include \"main.hlsl\"\n".into(),
            },
        ];
        let result = assemble(&files).unwrap();
        assert_eq!(result.matches("// 中文").count(), 1);
        assert!(result.contains("not recursively including main.hlsl"));
        assert!(result.contains("not re-including INC.HLSL"));
        assert!(result.contains("Can't find file missing"));
        assert!(
            assemble(&[SourceFile {
                name: "x".into(),
                contents: "#include broken".into()
            }])
            .is_err()
        );
    }
}
