use mado::nvim::redraw::{RedrawEvent, decode_redraw_notification};
use rmpv::Value;

#[test]
fn decodes_batched_linegrid_events() {
    let params = vec![
        Value::Array(vec![
            Value::from("grid_resize"),
            Value::Array(vec![Value::from(1), Value::from(80), Value::from(24)]),
        ]),
        Value::Array(vec![
            Value::from("grid_line"),
            Value::Array(vec![
                Value::from(1),
                Value::from(0),
                Value::from(0),
                Value::Array(vec![
                    Value::Array(vec![Value::from("日"), Value::from(4)]),
                    Value::Array(vec![Value::from("")]),
                    Value::Array(vec![Value::from(" "), Value::from(0), Value::from(3)]),
                ]),
                Value::from(false),
            ]),
        ]),
        Value::Array(vec![Value::from("flush"), Value::Array(vec![])]),
    ];

    let events = decode_redraw_notification(&params).unwrap();
    assert_eq!(events.len(), 3);
    assert!(matches!(
        events[0],
        RedrawEvent::GridResize {
            width: 80,
            height: 24,
            ..
        }
    ));
    let RedrawEvent::GridLine { cells, .. } = &events[1] else {
        panic!("expected grid_line");
    };
    assert_eq!(cells[0].text, "日");
    assert_eq!(cells[1].text, "");
    assert_eq!(cells[2].repeat, 3);
    assert_eq!(events[2], RedrawEvent::Flush);
}

#[test]
fn keeps_unknown_events_forward_compatible() {
    let params = vec![Value::Array(vec![
        Value::from("future_event"),
        Value::Array(vec![Value::from(42)]),
    ])];

    let events = decode_redraw_notification(&params).unwrap();
    assert!(matches!(
        &events[0],
        RedrawEvent::Unknown { name, .. } if name == "future_event"
    ));
}

#[test]
fn decodes_gui_options_and_cursor_styles() {
    let params = vec![
        Value::Array(vec![
            Value::from("option_set"),
            Value::Array(vec![Value::from("guifont"), Value::from("SF Mono:h16")]),
            Value::Array(vec![Value::from("linespace"), Value::from(2)]),
        ]),
        Value::Array(vec![
            Value::from("mode_info_set"),
            Value::Array(vec![
                Value::from(true),
                Value::Array(vec![Value::Map(vec![
                    (Value::from("cursor_shape"), Value::from("vertical")),
                    (Value::from("cell_percentage"), Value::from(25)),
                    (Value::from("blinkon"), Value::from(500)),
                ])]),
            ]),
        ]),
    ];

    let events = decode_redraw_notification(&params).unwrap();
    assert!(matches!(
        &events[0],
        RedrawEvent::OptionSet { name, value }
            if name == "guifont" && value.as_str() == Some("SF Mono:h16")
    ));
    assert!(matches!(
        &events[1],
        RedrawEvent::OptionSet { name, value }
            if name == "linespace" && value.as_i64() == Some(2)
    ));
    let RedrawEvent::ModeInfoSet { styles, .. } = &events[2] else {
        panic!("expected mode_info_set");
    };
    assert_eq!(styles[0].cell_percentage, 25);
    assert_eq!(styles[0].blink_on, 500);
}
