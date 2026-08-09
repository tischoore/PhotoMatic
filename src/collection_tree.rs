/// What kind of `nav_tree` node a clicked/right-clicked item's `lParam` represents. An event
/// leaf rides a positive id (see `event_tree.rs`); a collection leaf rides the *negative* of
/// its id instead, so the two never collide even though `events.id` and `collections.id` are
/// independent sequences that can both start at 1. A directory/type/root node carries no id
/// at all (`lParam` 0, NWG's default for a plain `insert_item`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeNodeKind {
    Event(i64),
    Collection(i64),
    Other,
}

/// Encodes a collection id as a tree item's `lParam`. `collections.id` (an `INTEGER PRIMARY
/// KEY`) is always >= 1, so this can never produce `0` and collide with a node that carries
/// no id at all.
pub fn encode_collection_param(id: i64) -> isize {
    -(id as isize)
}

/// Decodes a tree item's `lParam` back into what kind of node it represents — used by
/// `nav_tree_click`/`nav_tree_right_click` in place of the old "positive param means event"
/// check, now that collections also ride an id.
pub fn decode_param(param: isize) -> TreeNodeKind {
    if param > 0 {
        TreeNodeKind::Event(param as i64)
    } else if param < 0 {
        TreeNodeKind::Collection(-param as i64)
    } else {
        TreeNodeKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positive_param_decodes_as_an_event() {
        assert_eq!(decode_param(7), TreeNodeKind::Event(7));
    }

    #[test]
    fn negative_param_decodes_as_a_collection() {
        assert_eq!(decode_param(encode_collection_param(3)), TreeNodeKind::Collection(3));
    }

    #[test]
    fn zero_param_decodes_as_other() {
        assert_eq!(decode_param(0), TreeNodeKind::Other);
    }

    #[test]
    fn encode_collection_param_is_the_negative_of_the_id() {
        assert_eq!(encode_collection_param(5), -5);
    }
}
