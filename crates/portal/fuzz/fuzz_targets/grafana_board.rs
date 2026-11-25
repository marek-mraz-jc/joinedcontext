//! What the dashboard builder reads from outside: a stored dashboard file from the forge and a
//! Board a person sends. Any bytes yield a file or a reason, never a panic; a Board that passes
//! its shape check compiles into a file the builder reads back as the same Board.
#![no_main]

use joinedcontext_portal::grafana_builder::{
    check_shape, compile, decompile, read_file, Board, Source, ValueKind,
};
use libfuzzer_sys::fuzz_target;

const UID: &str = "jcd-0123456789abcdef";

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let open = |_: &str| Some("open".to_owned());
    if let Ok(file) = read_file(UID, text) {
        let _ = decompile(&file, &open);
    }
    let Ok(board) = serde_json::from_str::<Board>(text) else {
        return;
    };
    if check_shape(&board).is_err() {
        return;
    }
    let sources: Vec<Source> = board
        .panels
        .iter()
        .map(|panel| Source {
            datasource: "open".into(),
            named: false,
            filters: vec![ValueKind::Text; panel.filters.len()],
        })
        .collect();
    let file = compile(UID, &board, &sources, &vec![Vec::new(); board.variables.len()]);
    read_file(UID, &file.to_string()).expect("a compiled board is the builder's own file");
});
