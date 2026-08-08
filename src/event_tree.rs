use crate::db::models::{EventSummary, EventType};

/// The fixed display order of event types in the Left Navigation tree's Events node —
/// generation order (Tight Burst, Session, Multi-hour), same as `db::events::regenerate`.
const TYPES_IN_ORDER: [EventType; 3] = [EventType::TightBurst, EventType::Session, EventType::MultiHour];

/// One event leaf under a type node: the tree label (the event's title, defaulting to
/// its photo count) paired with its `events.id`, which rides on the tree item as its
/// `lParam` so a click can look the event back up without parsing the label.
#[derive(Debug, Clone, PartialEq)]
pub struct EventLeaf {
    pub id: i64,
    pub title: String,
}

/// One event-type node (Tight Burst/Session/Multi-hour) under the Events root, with a
/// display label ("Session (4)", following the File Type nodes' "ext (n)" convention)
/// and its events, in the order `events` was given.
#[derive(Debug, Clone, PartialEq)]
pub struct EventTypeNode {
    pub event_type: EventType,
    pub label: String,
    pub events: Vec<EventLeaf>,
}

/// Buckets `events` (expected chronologically ordered, as `ProjectDb::list_events` returns
/// them) into a fixed-order `EventTypeNode` per type — a type with zero events is omitted
/// entirely rather than shown empty, the same "absent, not zero" convention
/// `nav_tree::build` uses for a disabled File Type. Pure and GUI/DB-free so this shape is
/// unit-testable on its own.
pub fn build(events: &[EventSummary]) -> Vec<EventTypeNode> {
    TYPES_IN_ORDER
        .into_iter()
        .filter_map(|event_type| {
            let matching: Vec<&EventSummary> = events.iter().filter(|e| e.event_type == event_type).collect();
            if matching.is_empty() {
                return None;
            }
            let label = format!("{event_type} ({})", matching.len());
            let events = matching.into_iter().map(|e| EventLeaf { id: e.id, title: e.title.clone() }).collect();
            Some(EventTypeNode { event_type, label, events })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: i64, event_type: EventType, title: &str) -> EventSummary {
        EventSummary { id, event_type, title: title.to_string(), image_count: title.parse().unwrap_or(0) }
    }

    #[test]
    fn no_events_produces_no_nodes() {
        assert_eq!(build(&[]), vec![]);
    }

    #[test]
    fn a_type_with_no_events_is_omitted_entirely() {
        let nodes = build(&[summary(1, EventType::Session, "2")]);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].event_type, EventType::Session);
    }

    #[test]
    fn nodes_are_produced_in_tight_burst_session_multi_hour_order_regardless_of_input_order() {
        let events =
            [summary(1, EventType::MultiHour, "4"), summary(2, EventType::TightBurst, "3"), summary(3, EventType::Session, "2")];
        let nodes = build(&events);
        assert_eq!(
            nodes.iter().map(|n| n.event_type).collect::<Vec<_>>(),
            vec![EventType::TightBurst, EventType::Session, EventType::MultiHour]
        );
    }

    #[test]
    fn label_includes_the_event_count_following_the_file_type_convention() {
        let events = [summary(1, EventType::Session, "2"), summary(2, EventType::Session, "5")];
        let nodes = build(&events);
        assert_eq!(nodes[0].label, "Session (2)");
    }

    #[test]
    fn events_within_a_type_preserve_the_given_order() {
        let events = [summary(1, EventType::Session, "first"), summary(2, EventType::Session, "second")];
        let nodes = build(&events);
        assert_eq!(
            nodes[0].events,
            vec![EventLeaf { id: 1, title: "first".to_string() }, EventLeaf { id: 2, title: "second".to_string() }]
        );
    }
}
