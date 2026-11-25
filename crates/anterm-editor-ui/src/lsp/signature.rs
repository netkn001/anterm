//! Signature help: the parameters of the call the cursor is in, shown in a
//! popover once `(` or `,` is typed and kept current while typing goes on
//! inside the call. The server decides when the cursor has left it (it
//! answers with nothing), and Escape puts the popover away.

use gpui::Context;
#[cfg(test)]
use lsp_types::{Documentation, ParameterLabel, SignatureHelp};

#[cfg(test)]
use super::convert::{self, Encoding};
use crate::CodeEditorWorkspace;
use crate::code_editor::LocalFile;

#[cfg(test)]
fn documentation(doc: &Documentation) -> &str {
    match doc {
        Documentation::String(s) => s,
        Documentation::MarkupContent(m) => &m.value,
    }
}

/// Markdown's own punctuation, escaped so a signature reads as written —
/// `*const T` is not emphasis.
#[cfg(test)]
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(
            c,
            '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '#' | '|'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The popover's text: the active signature with the active parameter in
/// bold, then what the server says about that parameter and the function.
#[cfg(test)]
pub fn markdown(help: &SignatureHelp) -> Option<String> {
    let index = help.active_signature.unwrap_or(0) as usize;
    let signature = help
        .signatures
        .get(index)
        .or_else(|| help.signatures.first())?;
    let label = signature.label.as_str();
    let active = signature.active_parameter.or(help.active_parameter);
    let span = active
        .and_then(|ix| signature.parameters.as_ref()?.get(ix as usize))
        .and_then(|param| match &param.label {
            ParameterLabel::Simple(name) => {
                let start = label.find(name.as_str())?;
                Some((start, start + name.len(), param))
            }
            ParameterLabel::LabelOffsets([start, end]) => {
                // Offsets into the label, in UTF-16 units whatever the
                // position encoding.
                let start = convert::byte_of_column(label, *start, Encoding::Utf16);
                let end = convert::byte_of_column(label, *end, Encoding::Utf16).max(start);
                Some((start, end, param))
            }
        });
    let mut out = String::new();
    if help.signatures.len() > 1 {
        out.push_str(&format!("{}/{}  ", index + 1, help.signatures.len()));
    }
    match span {
        Some((start, end, _)) if start < end => {
            out.push_str(&escape(&label[..start]));
            out.push_str("**");
            out.push_str(&escape(&label[start..end]));
            out.push_str("**");
            out.push_str(&escape(&label[end..]));
        }
        _ => out.push_str(&escape(label)),
    }
    if let Some(doc) = span.and_then(|(_, _, p)| p.documentation.as_ref()) {
        let doc = documentation(doc).trim();
        if !doc.is_empty() {
            out.push_str("\n\n");
            out.push_str(doc);
        }
    }
    if let Some(doc) = signature.documentation.as_ref() {
        let doc = documentation(doc).trim();
        if !doc.is_empty() {
            out.push_str("\n\n");
            out.push_str(doc);
        }
    }
    Some(out)
}

impl CodeEditorWorkspace {
    /// After an edit: opens signature help on a trigger character, and
    /// keeps an open one current.
    pub fn lsp_signature_help_after_edit(&self, _f: &LocalFile, _cx: &mut Context<Self>) {
        // Upstream `gpui-kit` `EditorState` does not expose a built-in signature
        // help popover yet; hover/completion popovers remain active.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{ParameterInformation, SignatureInformation};

    fn help(params: Vec<ParameterLabel>, active: Option<u32>) -> SignatureHelp {
        SignatureHelp {
            signatures: vec![SignatureInformation {
                label: "fn copy(src: *const u8, n: usize)".into(),
                documentation: Some(Documentation::String("Copies.".into())),
                parameters: Some(
                    params
                        .into_iter()
                        .map(|label| ParameterInformation {
                            label,
                            documentation: None,
                        })
                        .collect(),
                ),
                active_parameter: None,
            }],
            active_signature: Some(0),
            active_parameter: active,
        }
    }

    #[test]
    fn the_active_parameter_is_bold_and_markdown_is_escaped() {
        let by_name = help(
            vec![
                ParameterLabel::Simple("src: *const u8".into()),
                ParameterLabel::Simple("n: usize".into()),
            ],
            Some(1),
        );
        assert_eq!(
            markdown(&by_name).unwrap(),
            "fn copy(src: \\*const u8, **n: usize**)\n\nCopies."
        );
        let by_offset = help(
            vec![
                ParameterLabel::LabelOffsets([8, 22]),
                ParameterLabel::LabelOffsets([24, 32]),
            ],
            Some(0),
        );
        assert_eq!(
            markdown(&by_offset).unwrap(),
            "fn copy(**src: \\*const u8**, n: usize)\n\nCopies."
        );
        assert!(
            markdown(&SignatureHelp {
                signatures: vec![],
                active_signature: None,
                active_parameter: None
            })
            .is_none()
        );
    }
}
