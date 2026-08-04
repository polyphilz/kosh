pub(super) const EMPTY_BLOCK_MARKER: &str = "<!-- kosh:block:empty -->";
pub(super) const CHILDREN_START_MARKER: &str = "<!-- kosh:children:start -->";
pub(super) const CHILDREN_END_MARKER: &str = "<!-- kosh:children:end -->";

pub(super) fn is_kosh_structure_marker(value: &str) -> bool {
    matches!(
        value.trim(),
        EMPTY_BLOCK_MARKER | CHILDREN_START_MARKER | CHILDREN_END_MARKER
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_only_exact_reserved_structure_comments() {
        for marker in [
            EMPTY_BLOCK_MARKER,
            CHILDREN_START_MARKER,
            CHILDREN_END_MARKER,
        ] {
            assert!(is_kosh_structure_marker(marker));
            assert!(is_kosh_structure_marker(&format!("\n{marker}\n")));
        }
        assert!(!is_kosh_structure_marker("<!-- kosh:block:empty-ish -->"));
        assert!(!is_kosh_structure_marker(
            "<!-- kosh:children:start --><script>"
        ));
    }
}
