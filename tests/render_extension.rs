use opurom_core::{Ir, Node, RenderingHook, Runtime};
use serde_json::{json, Map};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const COUNTER: &str = include_str!("fixtures/counter.opuir.json");

fn decoration_node() -> Node {
    Node {
        id: 99,
        kind: "Text".to_owned(),
        props: Map::from_iter([("value".to_owned(), json!("decoration"))]),
        children: Vec::new(),
        when: None,
        events: HashMap::new(),
    }
}

#[test]
fn unregistered_hook_keeps_display_list_byte_identical() {
    let runtime = Runtime::from_ir_json(COUNTER).unwrap();
    let rendered = runtime.display_list();
    let parsed = Ir::parse(COUNTER).unwrap();
    assert_eq!(rendered, parsed.tree);
    assert_eq!(rendered.props, parsed.tree.props);
    assert_eq!(rendered.children, parsed.tree.children);
}

#[test]
fn registered_hook_decorates_the_display_list() {
    #[derive(Debug)]
    struct Append;

    impl RenderingHook for Append {
        fn decorate(&self, list: &Node) -> Node {
            let mut decorated = list.clone();
            decorated.children.push(decoration_node());
            decorated
        }
    }

    let runtime = Runtime::from_ir_json(COUNTER)
        .unwrap()
        .with_rendering_hook(Arc::new(Append));
    let rendered = runtime.display_list();
    let parsed = Ir::parse(COUNTER).unwrap();

    assert_ne!(rendered, parsed.tree);
    assert_eq!(rendered.children.len(), parsed.tree.children.len() + 1);
    assert_eq!(rendered.children.last().unwrap().id, 99);
    // Only the decoration changed; the parsed children survive untouched.
    assert_eq!(
        &rendered.children[..parsed.tree.children.len()],
        &parsed.tree.children[..]
    );
}

#[test]
fn observing_hook_keeps_the_display_list_unchanged() {
    #[derive(Debug)]
    struct Observer {
        seen: Arc<AtomicUsize>,
    }

    impl RenderingHook for Observer {
        fn observe(&self, _list: &Node) {
            self.seen.fetch_add(1, Ordering::SeqCst);
        }
    }

    let seen = Arc::new(AtomicUsize::new(0));
    let runtime = Runtime::from_ir_json(COUNTER)
        .unwrap()
        .with_rendering_hook(Arc::new(Observer {
            seen: Arc::clone(&seen),
        }));
    let rendered = runtime.display_list();
    let parsed = Ir::parse(COUNTER).unwrap();

    assert_eq!(rendered, parsed.tree);
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}
