use anyhow::{Context, Result, bail};
use rmpv::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridCell {
    pub text: String,
    pub highlight_id: Option<u64>,
    pub repeat: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HighlightAttrs {
    pub foreground: Option<u32>,
    pub background: Option<u32>,
    pub special: Option<u32>,
    pub reverse: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub undercurl: bool,
    pub strikethrough: bool,
    pub underdouble: bool,
    pub underdotted: bool,
    pub underdashed: bool,
    pub overline: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CursorShape {
    #[default]
    Block,
    Vertical,
    Horizontal,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CursorStyle {
    pub shape: CursorShape,
    pub cell_percentage: u64,
    pub blink_wait: u64,
    pub blink_on: u64,
    pub blink_off: u64,
    pub attr_id: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RedrawEvent {
    GridResize {
        grid: u64,
        width: u64,
        height: u64,
    },
    GridLine {
        grid: u64,
        row: u64,
        col_start: u64,
        cells: Vec<GridCell>,
        wrap: bool,
    },
    GridClear {
        grid: u64,
    },
    GridCursorGoto {
        grid: u64,
        row: u64,
        col: u64,
    },
    GridScroll {
        grid: u64,
        top: u64,
        bottom: u64,
        left: u64,
        right: u64,
        rows: i64,
        cols: i64,
    },
    DefaultColorsSet {
        foreground: i64,
        background: i64,
        special: i64,
    },
    HighlightDefine {
        id: u64,
        attrs: HighlightAttrs,
    },
    ModeChange {
        mode: String,
        mode_index: u64,
    },
    ModeInfoSet {
        cursor_style_enabled: bool,
        styles: Vec<CursorStyle>,
    },
    OptionSet {
        name: String,
        value: Value,
    },
    Flush,
    Unknown {
        name: String,
        args: Vec<Value>,
    },
}

pub fn decode_redraw_notification(params: &[Value]) -> Result<Vec<RedrawEvent>> {
    let mut decoded = Vec::new();

    for raw_event in params {
        let event = raw_event
            .as_array()
            .context("redraw event must be an array")?;
        let name = event
            .first()
            .and_then(Value::as_str)
            .context("redraw event name must be a string")?;

        for raw_args in &event[1..] {
            let args = raw_args
                .as_array()
                .with_context(|| format!("{name} arguments must be an array"))?;
            decoded.push(decode_event(name, args)?);
        }
    }

    Ok(decoded)
}

fn decode_event(name: &str, args: &[Value]) -> Result<RedrawEvent> {
    match name {
        "grid_resize" => Ok(RedrawEvent::GridResize {
            grid: number(args, 0, name)?,
            width: number(args, 1, name)?,
            height: number(args, 2, name)?,
        }),
        "grid_line" => Ok(RedrawEvent::GridLine {
            grid: number(args, 0, name)?,
            row: number(args, 1, name)?,
            col_start: number(args, 2, name)?,
            cells: decode_cells(args.get(3).context("grid_line is missing cells")?)?,
            wrap: args
                .get(4)
                .and_then(Value::as_bool)
                .context("grid_line wrap must be a boolean")?,
        }),
        "grid_clear" => Ok(RedrawEvent::GridClear {
            grid: number(args, 0, name)?,
        }),
        "grid_cursor_goto" => Ok(RedrawEvent::GridCursorGoto {
            grid: number(args, 0, name)?,
            row: number(args, 1, name)?,
            col: number(args, 2, name)?,
        }),
        "grid_scroll" => Ok(RedrawEvent::GridScroll {
            grid: number(args, 0, name)?,
            top: number(args, 1, name)?,
            bottom: number(args, 2, name)?,
            left: number(args, 3, name)?,
            right: number(args, 4, name)?,
            rows: signed_number(args, 5, name)?,
            cols: signed_number(args, 6, name)?,
        }),
        "default_colors_set" => Ok(RedrawEvent::DefaultColorsSet {
            foreground: signed_number(args, 0, name)?,
            background: signed_number(args, 1, name)?,
            special: signed_number(args, 2, name)?,
        }),
        "hl_attr_define" => Ok(RedrawEvent::HighlightDefine {
            id: number(args, 0, name)?,
            attrs: decode_highlight(
                args.get(1)
                    .context("hl_attr_define is missing RGB attributes")?,
            )?,
        }),
        "mode_change" => Ok(RedrawEvent::ModeChange {
            mode: args
                .first()
                .and_then(Value::as_str)
                .context("mode_change mode must be a string")?
                .to_owned(),
            mode_index: number(args, 1, name)?,
        }),
        "mode_info_set" => Ok(RedrawEvent::ModeInfoSet {
            cursor_style_enabled: args
                .first()
                .and_then(Value::as_bool)
                .context("mode_info_set enabled flag must be a boolean")?,
            styles: decode_cursor_styles(
                args.get(1)
                    .context("mode_info_set is missing cursor styles")?,
            )?,
        }),
        "option_set" => Ok(RedrawEvent::OptionSet {
            name: args
                .first()
                .and_then(Value::as_str)
                .context("option_set name must be a string")?
                .to_owned(),
            value: args.get(1).context("option_set is missing value")?.clone(),
        }),
        "flush" => Ok(RedrawEvent::Flush),
        _ => Ok(RedrawEvent::Unknown {
            name: name.to_owned(),
            args: args.to_vec(),
        }),
    }
}

fn decode_highlight(value: &Value) -> Result<HighlightAttrs> {
    let entries = value
        .as_map()
        .context("highlight RGB attributes must be a map")?;
    let color = |name: &str| {
        entries
            .iter()
            .find(|(key, _)| key.as_str() == Some(name))
            .and_then(|(_, value)| value.as_u64())
            .map(|value| value as u32)
    };
    let flag = |name: &str| {
        entries
            .iter()
            .find(|(key, _)| key.as_str() == Some(name))
            .and_then(|(_, value)| value.as_bool())
            .unwrap_or(false)
    };

    Ok(HighlightAttrs {
        foreground: color("foreground"),
        background: color("background"),
        special: color("special"),
        reverse: flag("reverse"),
        bold: flag("bold"),
        italic: flag("italic"),
        underline: flag("underline"),
        undercurl: flag("undercurl"),
        strikethrough: flag("strikethrough"),
        underdouble: flag("underdouble"),
        underdotted: flag("underdotted"),
        underdashed: flag("underdashed"),
        overline: flag("overline"),
    })
}

fn decode_cursor_styles(value: &Value) -> Result<Vec<CursorStyle>> {
    value
        .as_array()
        .context("mode_info_set styles must be an array")?
        .iter()
        .map(|value| {
            let entries = value.as_map().context("cursor style must be a map")?;
            let get = |name: &str| {
                entries
                    .iter()
                    .find(|(key, _)| key.as_str() == Some(name))
                    .map(|(_, value)| value)
            };
            let shape = match get("cursor_shape").and_then(Value::as_str) {
                Some("vertical") => CursorShape::Vertical,
                Some("horizontal") => CursorShape::Horizontal,
                _ => CursorShape::Block,
            };
            Ok(CursorStyle {
                shape,
                cell_percentage: get("cell_percentage")
                    .and_then(Value::as_u64)
                    .unwrap_or(100),
                blink_wait: get("blinkwait").and_then(Value::as_u64).unwrap_or(0),
                blink_on: get("blinkon").and_then(Value::as_u64).unwrap_or(0),
                blink_off: get("blinkoff").and_then(Value::as_u64).unwrap_or(0),
                attr_id: get("attr_id").and_then(Value::as_u64),
            })
        })
        .collect()
}

fn decode_cells(value: &Value) -> Result<Vec<GridCell>> {
    value
        .as_array()
        .context("grid_line cells must be an array")?
        .iter()
        .map(|raw_cell| {
            let cell = raw_cell.as_array().context("grid cell must be an array")?;
            if cell.len() > 3 {
                bail!("grid cell has more than three fields")
            }
            Ok(GridCell {
                text: cell
                    .first()
                    .and_then(Value::as_str)
                    .context("grid cell text must be a string")?
                    .to_owned(),
                highlight_id: cell
                    .get(1)
                    .map(|value| {
                        value
                            .as_u64()
                            .context("grid cell highlight id must be an integer")
                    })
                    .transpose()?,
                repeat: cell
                    .get(2)
                    .map(|value| {
                        value
                            .as_u64()
                            .context("grid cell repeat must be an integer")
                    })
                    .transpose()?
                    .unwrap_or(1),
            })
        })
        .collect()
}

fn number(args: &[Value], index: usize, event: &str) -> Result<u64> {
    args.get(index)
        .and_then(Value::as_u64)
        .with_context(|| format!("{event} argument {index} must be an unsigned integer"))
}

fn signed_number(args: &[Value], index: usize, event: &str) -> Result<i64> {
    args.get(index)
        .and_then(Value::as_i64)
        .with_context(|| format!("{event} argument {index} must be an integer"))
}
