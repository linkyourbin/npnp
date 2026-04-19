#[path = "schlib.rs"]
#[allow(dead_code)]
mod old;

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use serde_json::Value;

use crate::error::Result;

const WHITE_BGR: i32 = 0xFFFFFF;
const BODY_LINE_WIDTH_INDEX: i32 = 1;
const GRAPHIC_LINE_WIDTH_INDEX: i32 = 1;
const PIN_LENGTH_UNITS: f64 = 20.0;

pub fn write_schlib_from_payload(
    payload: &Value,
    component_name: &str,
    output_path: &Path,
) -> Result<()> {
    let component = build_component(payload, component_name)?;
    write_schlib(&component, output_path)
}

fn build_component(payload: &Value, component_name: &str) -> Result<Component> {
    let rows = old::parse_easyeda_rows(payload)?;
    let mut parts: Vec<PartRaw> = Vec::new();
    let mut current_part_index = None;
    let mut has_part_rows = false;
    let mut attr_by_parent: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut global_bounds = old::OptionalBounds::default();

    for row in &rows {
        let Some(row_type) = row.first().and_then(Value::as_str) else {
            continue;
        };
        match row_type.trim().to_ascii_uppercase().as_str() {
            "PART" => {
                has_part_rows = true;
                let bounds = old::part_bounds_from_row(row);
                if let Some(bounds) = bounds {
                    global_bounds.update_x(bounds.min_x_units, bounds.max_x_units);
                    global_bounds.update_y(bounds.min_y_units, bounds.max_y_units);
                }
                let owner_part_id = parts.len() as i32 + 1;
                parts.push(PartRaw::new(owner_part_id, bounds));
                current_part_index = Some(parts.len() - 1);
            }
            "ATTR" => {
                let parent = old::row_string(row, 2);
                let key = old::row_string(row, 3);
                if parent.trim().is_empty() || key.trim().is_empty() {
                    continue;
                }
                let key_upper = key.trim().to_ascii_uppercase();
                let attrs = attr_by_parent.entry(parent.trim().to_string()).or_default();
                attrs.insert(key_upper.clone(), old::row_string(row, 4));
                attrs.insert(
                    format!("{key_upper}__VISIBLE"),
                    old::row_bool(row, 6, true).to_string(),
                );
            }
            "PIN" => {
                let part_index = ensure_current_part_index(&mut parts, &mut current_part_index);
                let owner_part_id = parts[part_index].owner_part_id;
                let pin = PinRaw {
                    id: old::row_string(row, 1),
                    x_units: old::row_f64(row, 4, 0.0),
                    y_units: old::row_f64(row, 5, 0.0),
                    length_units: old::row_f64(row, 6, PIN_LENGTH_UNITS),
                    rotation_degrees: old::row_f64(row, 7, 0.0),
                    owner_part_id,
                };
                if pin.id.trim().is_empty() {
                    continue;
                }
                let angle = old::normalize_angle(pin.rotation_degrees);
                let (dx, dy) = if !(45.0..315.0).contains(&angle) {
                    (pin.length_units, 0.0)
                } else if angle < 135.0 {
                    (0.0, pin.length_units)
                } else if angle < 225.0 {
                    (-pin.length_units, 0.0)
                } else {
                    (0.0, -pin.length_units)
                };
                let min_x = pin.x_units.min(pin.x_units + dx);
                let max_x = pin.x_units.max(pin.x_units + dx);
                let min_y = pin.y_units.min(pin.y_units + dy);
                let max_y = pin.y_units.max(pin.y_units + dy);
                parts[part_index].bounds.update_x(min_x, max_x);
                parts[part_index].bounds.update_y(min_y, max_y);
                parts[part_index].pins.push(pin);
                global_bounds.update_x(min_x, max_x);
                global_bounds.update_y(min_y, max_y);
            }
            "RECT" => {
                let part_index = ensure_current_part_index(&mut parts, &mut current_part_index);
                let owner_part_id = parts[part_index].owner_part_id;
                let rect = RectRaw {
                    x1_units: old::row_f64(row, 2, 0.0),
                    y1_units: old::row_f64(row, 3, 0.0),
                    x2_units: old::row_f64(row, 4, 0.0),
                    y2_units: old::row_f64(row, 5, 0.0),
                    owner_part_id,
                };
                let min_x = rect.x1_units.min(rect.x2_units);
                let max_x = rect.x1_units.max(rect.x2_units);
                let min_y = rect.y1_units.min(rect.y2_units);
                let max_y = rect.y1_units.max(rect.y2_units);
                parts[part_index].bounds.update_x(min_x, max_x);
                parts[part_index].bounds.update_y(min_y, max_y);
                parts[part_index].rectangles.push(rect);
                global_bounds.update_x(min_x, max_x);
                global_bounds.update_y(min_y, max_y);
            }
            "POLY" | "POLYGON" | "PATH" => {
                let Some(shape) = row.get(2) else {
                    continue;
                };
                let points = parse_path_raw_points(shape);
                if points.len() < 2 {
                    continue;
                }
                let part_index = ensure_current_part_index(&mut parts, &mut current_part_index);
                let owner_part_id = parts[part_index].owner_part_id;
                for point in &points {
                    parts[part_index]
                        .bounds
                        .update_x(point.x_units, point.x_units);
                    parts[part_index]
                        .bounds
                        .update_y(point.y_units, point.y_units);
                    global_bounds.update_x(point.x_units, point.x_units);
                    global_bounds.update_y(point.y_units, point.y_units);
                }
                parts[part_index].polylines.push(PolylineRaw {
                    points,
                    owner_part_id,
                });
            }
            "LINE" => {
                let points = vec![
                    PointUnits {
                        x_units: old::row_f64(row, 2, 0.0),
                        y_units: old::row_f64(row, 3, 0.0),
                    },
                    PointUnits {
                        x_units: old::row_f64(row, 4, 0.0),
                        y_units: old::row_f64(row, 5, 0.0),
                    },
                ];
                let part_index = ensure_current_part_index(&mut parts, &mut current_part_index);
                let owner_part_id = parts[part_index].owner_part_id;
                for point in &points {
                    parts[part_index]
                        .bounds
                        .update_x(point.x_units, point.x_units);
                    parts[part_index]
                        .bounds
                        .update_y(point.y_units, point.y_units);
                    global_bounds.update_x(point.x_units, point.x_units);
                    global_bounds.update_y(point.y_units, point.y_units);
                }
                parts[part_index].polylines.push(PolylineRaw {
                    points,
                    owner_part_id,
                });
            }
            "ARC" => {
                let part_index = ensure_current_part_index(&mut parts, &mut current_part_index);
                let owner_part_id = parts[part_index].owner_part_id;
                let arc = ArcRaw {
                    start: PointUnits {
                        x_units: old::row_f64(row, 2, 0.0),
                        y_units: old::row_f64(row, 3, 0.0),
                    },
                    mid: PointUnits {
                        x_units: old::row_f64(row, 4, 0.0),
                        y_units: old::row_f64(row, 5, 0.0),
                    },
                    end: PointUnits {
                        x_units: old::row_f64(row, 6, 0.0),
                        y_units: old::row_f64(row, 7, 0.0),
                    },
                    owner_part_id,
                };
                for point in [arc.start, arc.mid, arc.end] {
                    parts[part_index]
                        .bounds
                        .update_x(point.x_units, point.x_units);
                    parts[part_index]
                        .bounds
                        .update_y(point.y_units, point.y_units);
                    global_bounds.update_x(point.x_units, point.x_units);
                    global_bounds.update_y(point.y_units, point.y_units);
                }
                parts[part_index].arcs.push(arc);
            }
            "CIRCLE" => {
                let r = old::row_f64(row, 4, 0.0).abs();
                if r <= 0.000001 {
                    continue;
                }
                let part_index = ensure_current_part_index(&mut parts, &mut current_part_index);
                let owner_part_id = parts[part_index].owner_part_id;
                let filled = r <= 1.0 + f64::EPSILON;
                let ellipse = EllipseRaw {
                    center_x_units: old::row_f64(row, 2, 0.0),
                    center_y_units: old::row_f64(row, 3, 0.0),
                    radius_x_units: r,
                    radius_y_units: r,
                    owner_part_id,
                    is_filled: filled,
                    is_transparent: !filled,
                };
                update_ellipse_bounds(&mut parts[part_index].bounds, &ellipse);
                update_ellipse_bounds(&mut global_bounds, &ellipse);
                parts[part_index].ellipses.push(ellipse);
            }
            "ELLIPSE" => {
                let rx = old::row_f64(row, 4, 0.0).abs();
                let ry = old::row_f64(row, 5, 0.0).abs();
                if rx <= 0.000001 || ry <= 0.000001 {
                    continue;
                }
                let part_index = ensure_current_part_index(&mut parts, &mut current_part_index);
                let owner_part_id = parts[part_index].owner_part_id;
                let filled = rx.max(ry) <= 1.0 + f64::EPSILON;
                let ellipse = EllipseRaw {
                    center_x_units: old::row_f64(row, 2, 0.0),
                    center_y_units: old::row_f64(row, 3, 0.0),
                    radius_x_units: rx,
                    radius_y_units: ry,
                    owner_part_id,
                    is_filled: filled,
                    is_transparent: !filled,
                };
                update_ellipse_bounds(&mut parts[part_index].bounds, &ellipse);
                update_ellipse_bounds(&mut global_bounds, &ellipse);
                parts[part_index].ellipses.push(ellipse);
            }
            "TEXT" => {
                let text = normalize_text_value(&old::row_string(row, 5));
                if text.is_empty() {
                    continue;
                }
                let part_index = ensure_current_part_index(&mut parts, &mut current_part_index);
                let owner_part_id = parts[part_index].owner_part_id;
                let label = TextRaw {
                    text,
                    x_units: old::row_f64(row, 2, 0.0),
                    y_units: old::row_f64(row, 3, 0.0),
                    rotation_degrees: old::row_f64(row, 4, 0.0),
                    owner_part_id,
                };
                parts[part_index]
                    .bounds
                    .update_x(label.x_units, label.x_units);
                parts[part_index]
                    .bounds
                    .update_y(label.y_units, label.y_units);
                global_bounds.update_x(label.x_units, label.x_units);
                global_bounds.update_y(label.y_units, label.y_units);
                parts[part_index].labels.push(label);
            }
            _ => {}
        }
    }

    if parts.is_empty() {
        parts.push(PartRaw::new(1, None));
    }

    let mut component = Component {
        name: normalize_component_name(component_name),
        description: "Generated from EasyEDA symbol".to_string(),
        part_count: parts.len().max(1),
        pins: Vec::new(),
        rectangles: Vec::new(),
        polylines: Vec::new(),
        arcs: Vec::new(),
        ellipses: Vec::new(),
        labels: Vec::new(),
    };
    let has_original_graphics = parts.iter().any(PartRaw::has_graphics);

    if !has_original_graphics && !has_part_rows {
        let layout_pins: Vec<old::PinRaw> = parts[0]
            .pins
            .iter()
            .map(|pin| old::PinRaw {
                id: pin.id.clone(),
                x_units: pin.x_units,
                y_units: pin.y_units,
                length_units: pin.length_units,
                rotation_degrees: pin.rotation_degrees,
            })
            .collect();
        let (placed_pins, laid_out_rect) = old::layout_pins(&layout_pins, &attr_by_parent);
        if !placed_pins.is_empty() {
            component.rectangles.push(Rectangle {
                corner1: old::CoordPoint::from_symbol_units(
                    laid_out_rect.x1_units,
                    laid_out_rect.height_units() - laid_out_rect.y1_units,
                ),
                corner2: old::CoordPoint::from_symbol_units(
                    laid_out_rect.x2_units,
                    laid_out_rect.height_units() - laid_out_rect.y2_units,
                ),
                color_bgr: old::BORDER_BGR,
                fill_color_bgr: old::FILL_BGR,
                is_filled: true,
                is_transparent: false,
                line_width_index: BODY_LINE_WIDTH_INDEX,
                owner_part_id: 1,
            });
            for pin in placed_pins {
                component.pins.push(Pin {
                    designator: pin.designator,
                    name: pin.name,
                    location: old::CoordPoint::from_symbol_units(
                        pin.x_units,
                        laid_out_rect.height_units() - pin.y_units,
                    ),
                    length_raw: old::raw_from_symbol_units(pin.length_units),
                    orientation: pin_orientation_from_easyeda_rotation(pin.rotation_degrees),
                    show_name: pin.show_name,
                    show_designator: pin.show_designator,
                    color_bgr: old::RED_BGR,
                    owner_part_id: 1,
                    owner_part_display_mode: 0,
                });
            }
            return Ok(component);
        }
    }

    let mut has_any_body = false;
    for part in &parts {
        let complex = part.has_complex_body();
        if !part.rectangles.is_empty() {
            for rect in &part.rectangles {
                component.rectangles.push(Rectangle {
                    corner1: old::CoordPoint::from_symbol_units(rect.x1_units, rect.y1_units),
                    corner2: old::CoordPoint::from_symbol_units(rect.x2_units, rect.y2_units),
                    color_bgr: old::BORDER_BGR,
                    fill_color_bgr: if complex { WHITE_BGR } else { old::FILL_BGR },
                    is_filled: !complex,
                    is_transparent: complex,
                    line_width_index: BODY_LINE_WIDTH_INDEX,
                    owner_part_id: rect.owner_part_id,
                });
                has_any_body = true;
            }
        } else if let Some(bounds) = part.bounds.finish() {
            if !part.pins.is_empty() && !part.has_graphics() {
                component.rectangles.push(Rectangle {
                    corner1: old::CoordPoint::from_symbol_units(
                        bounds.min_x_units,
                        bounds.max_y_units,
                    ),
                    corner2: old::CoordPoint::from_symbol_units(
                        bounds.max_x_units,
                        bounds.min_y_units,
                    ),
                    color_bgr: old::BORDER_BGR,
                    fill_color_bgr: if complex { WHITE_BGR } else { old::FILL_BGR },
                    is_filled: !complex,
                    is_transparent: complex,
                    line_width_index: BODY_LINE_WIDTH_INDEX,
                    owner_part_id: part.owner_part_id,
                });
                has_any_body = true;
            }
        }
        for polyline in &part.polylines {
            component.polylines.push(Polyline {
                points: polyline
                    .points
                    .iter()
                    .map(|point| old::CoordPoint::from_symbol_units(point.x_units, point.y_units))
                    .collect(),
                color_bgr: old::RED_BGR,
                line_width_index: GRAPHIC_LINE_WIDTH_INDEX,
                owner_part_id: polyline.owner_part_id,
            });
            has_any_body = true;
        }
        for arc in &part.arcs {
            if let Some(converted) = arc_from_raw(arc) {
                component.arcs.push(converted);
            } else {
                component.polylines.push(Polyline {
                    points: vec![
                        old::CoordPoint::from_symbol_units(arc.start.x_units, arc.start.y_units),
                        old::CoordPoint::from_symbol_units(arc.mid.x_units, arc.mid.y_units),
                        old::CoordPoint::from_symbol_units(arc.end.x_units, arc.end.y_units),
                    ],
                    color_bgr: old::RED_BGR,
                    line_width_index: GRAPHIC_LINE_WIDTH_INDEX,
                    owner_part_id: arc.owner_part_id,
                });
            }
            has_any_body = true;
        }
        for ellipse in &part.ellipses {
            component.ellipses.push(Ellipse {
                center: old::CoordPoint::from_symbol_units(
                    ellipse.center_x_units,
                    ellipse.center_y_units,
                ),
                radius_x_raw: old::raw_from_symbol_units(ellipse.radius_x_units),
                radius_y_raw: old::raw_from_symbol_units(ellipse.radius_y_units),
                color_bgr: old::RED_BGR,
                fill_color_bgr: if ellipse.is_filled {
                    old::RED_BGR
                } else {
                    WHITE_BGR
                },
                is_filled: ellipse.is_filled,
                is_transparent: ellipse.is_transparent,
                line_width_index: GRAPHIC_LINE_WIDTH_INDEX,
                owner_part_id: ellipse.owner_part_id,
            });
            has_any_body = true;
        }
        for label in &part.labels {
            component.labels.push(Label {
                text: label.text.clone(),
                location: old::CoordPoint::from_symbol_units(label.x_units, label.y_units),
                orientation: text_orientation_from_rotation(label.rotation_degrees),
                color_bgr: old::RED_BGR,
                owner_part_id: label.owner_part_id,
            });
            has_any_body = true;
        }
        for (pin_index, pin) in part.pins.iter().enumerate() {
            let attrs = attr_by_parent.get(&pin.id);
            let designator = old::safe_attr(attrs, "NUMBER")
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| (pin_index + 1).to_string());
            let name = old::safe_attr(attrs, "NAME")
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| designator.clone());
            let source_length_units = if pin.length_units > 0.000001 {
                pin.length_units
            } else {
                10.0
            };
            let export_length_units = PIN_LENGTH_UNITS;
            let show_name = old::safe_attr_flag(attrs, "NAME", !name.trim().is_empty());
            let show_designator = old::safe_attr_flag(attrs, "NUMBER", true);
            let (location_x_units, location_y_units) = pin_inner_location_from_easyeda(
                pin.x_units,
                pin.y_units,
                source_length_units,
                pin.rotation_degrees,
            );
            let location = old::CoordPoint::from_symbol_units(location_x_units, location_y_units);
            let orientation = old::pin_orientation_from_rotation(pin.rotation_degrees);
            component.pins.push(Pin {
                designator,
                name: name.clone(),
                location,
                length_raw: old::raw_from_symbol_units(export_length_units),
                orientation,
                show_name,
                show_designator,
                color_bgr: old::RED_BGR,
                owner_part_id: pin.owner_part_id,
                owner_part_display_mode: 0,
            });
        }
    }

    if !has_any_body {
        if let Some(bounds) = global_bounds.finish() {
            component.rectangles.push(Rectangle {
                corner1: old::CoordPoint::from_symbol_units(bounds.min_x_units, bounds.max_y_units),
                corner2: old::CoordPoint::from_symbol_units(bounds.max_x_units, bounds.min_y_units),
                color_bgr: old::BORDER_BGR,
                fill_color_bgr: WHITE_BGR,
                is_filled: false,
                is_transparent: true,
                line_width_index: BODY_LINE_WIDTH_INDEX,
                owner_part_id: 1,
            });
        }
    }

    Ok(component)
}

fn update_ellipse_bounds(bounds: &mut old::OptionalBounds, ellipse: &EllipseRaw) {
    bounds.update_x(
        ellipse.center_x_units - ellipse.radius_x_units,
        ellipse.center_x_units + ellipse.radius_x_units,
    );
    bounds.update_y(
        ellipse.center_y_units - ellipse.radius_y_units,
        ellipse.center_y_units + ellipse.radius_y_units,
    );
}

fn normalize_text_value(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn ensure_current_part_index(parts: &mut Vec<PartRaw>, current: &mut Option<usize>) -> usize {
    if current.is_none() {
        parts.push(PartRaw::new(1, None));
        *current = Some(0);
    }
    current.expect("part index")
}

fn parse_path_raw_points(shape: &Value) -> Vec<PointUnits> {
    match shape {
        Value::Array(values) => parse_path_array_points(values),
        Value::String(text) => parse_svg_path_points(text),
        _ => Vec::new(),
    }
}

fn parse_path_array_points(values: &[Value]) -> Vec<PointUnits> {
    let mut points = Vec::new();
    let mut index = 0usize;
    let mut start = None;
    let mut current = None;
    while index < values.len() {
        match &values[index] {
            Value::String(command) => {
                let cmd = command.trim().to_ascii_uppercase();
                index += 1;
                match cmd.as_str() {
                    "Z" | "CLOSE" => {
                        if let (Some(first), Some(last)) = (start, current) {
                            if !same_point(first, last) {
                                add_path_point(&mut points, first.x_units, first.y_units);
                                current = Some(first);
                            }
                        }
                    }
                    "M" | "L" => {
                        while index + 1 < values.len() {
                            let Some(x) = values.get(index).and_then(old::value_as_f64) else {
                                break;
                            };
                            let Some(y) = values.get(index + 1).and_then(old::value_as_f64) else {
                                break;
                            };
                            add_path_point(&mut points, x, y);
                            current = Some(PointUnits {
                                x_units: x,
                                y_units: y,
                            });
                            if start.is_none() {
                                start = current;
                            }
                            index += 2;
                        }
                    }
                    "H" => {
                        while index < values.len() {
                            let Some(x) = values.get(index).and_then(old::value_as_f64) else {
                                break;
                            };
                            let y = current.map_or(0.0, |point| point.y_units);
                            add_path_point(&mut points, x, y);
                            current = Some(PointUnits {
                                x_units: x,
                                y_units: y,
                            });
                            if start.is_none() {
                                start = current;
                            }
                            index += 1;
                        }
                    }
                    "V" => {
                        while index < values.len() {
                            let Some(y) = values.get(index).and_then(old::value_as_f64) else {
                                break;
                            };
                            let x = current.map_or(0.0, |point| point.x_units);
                            add_path_point(&mut points, x, y);
                            current = Some(PointUnits {
                                x_units: x,
                                y_units: y,
                            });
                            if start.is_none() {
                                start = current;
                            }
                            index += 1;
                        }
                    }
                    "ARC" | "A" => {
                        if index + 2 < values.len() {
                            if let (Some(x), Some(y)) = (
                                values.get(index + 1).and_then(old::value_as_f64),
                                values.get(index + 2).and_then(old::value_as_f64),
                            ) {
                                add_path_point(&mut points, x, y);
                                current = Some(PointUnits {
                                    x_units: x,
                                    y_units: y,
                                });
                                if start.is_none() {
                                    start = current;
                                }
                                index += 3;
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {
                if index + 1 < values.len() {
                    if let (Some(x), Some(y)) = (
                        values.get(index).and_then(old::value_as_f64),
                        values.get(index + 1).and_then(old::value_as_f64),
                    ) {
                        add_path_point(&mut points, x, y);
                        current = Some(PointUnits {
                            x_units: x,
                            y_units: y,
                        });
                        if start.is_none() {
                            start = current;
                        }
                        index += 2;
                        continue;
                    }
                }
                index += 1;
            }
        }
    }
    points
}
fn parse_svg_path_points(text: &str) -> Vec<PointUnits> {
    let tokens = tokenize_svg_path(text);
    let mut points = Vec::new();
    let mut index = 0usize;
    let mut command = 'M';
    let mut current = PointUnits {
        x_units: 0.0,
        y_units: 0.0,
    };
    let mut start = None;
    while index < tokens.len() {
        if let Some(letter) = tokens[index]
            .chars()
            .next()
            .filter(|ch| tokens[index].len() == 1 && ch.is_ascii_alphabetic())
        {
            command = letter;
            index += 1;
            if matches!(command, 'Z' | 'z') {
                if let Some(first) = start {
                    if !same_point(first, current) {
                        add_path_point(&mut points, first.x_units, first.y_units);
                        current = first;
                    }
                }
                continue;
            }
        }
        match command {
            'M' | 'm' => {
                if index + 1 >= tokens.len() {
                    break;
                }
                let Some(mut x) = tokens[index].parse::<f64>().ok() else {
                    break;
                };
                let Some(mut y) = tokens[index + 1].parse::<f64>().ok() else {
                    break;
                };
                if command == 'm' {
                    x += current.x_units;
                    y += current.y_units;
                }
                add_path_point(&mut points, x, y);
                current = PointUnits {
                    x_units: x,
                    y_units: y,
                };
                if start.is_none() {
                    start = Some(current);
                }
                command = if command == 'm' { 'l' } else { 'L' };
                index += 2;
            }
            'L' | 'l' => {
                if index + 1 >= tokens.len() {
                    break;
                }
                let Some(mut x) = tokens[index].parse::<f64>().ok() else {
                    break;
                };
                let Some(mut y) = tokens[index + 1].parse::<f64>().ok() else {
                    break;
                };
                if command == 'l' {
                    x += current.x_units;
                    y += current.y_units;
                }
                add_path_point(&mut points, x, y);
                current = PointUnits {
                    x_units: x,
                    y_units: y,
                };
                if start.is_none() {
                    start = Some(current);
                }
                index += 2;
            }
            'H' | 'h' => {
                let Some(mut x) = tokens
                    .get(index)
                    .and_then(|token| token.parse::<f64>().ok())
                else {
                    break;
                };
                if command == 'h' {
                    x += current.x_units;
                }
                add_path_point(&mut points, x, current.y_units);
                current = PointUnits {
                    x_units: x,
                    y_units: current.y_units,
                };
                if start.is_none() {
                    start = Some(current);
                }
                index += 1;
            }
            'V' | 'v' => {
                let Some(mut y) = tokens
                    .get(index)
                    .and_then(|token| token.parse::<f64>().ok())
                else {
                    break;
                };
                if command == 'v' {
                    y += current.y_units;
                }
                add_path_point(&mut points, current.x_units, y);
                current = PointUnits {
                    x_units: current.x_units,
                    y_units: y,
                };
                if start.is_none() {
                    start = Some(current);
                }
                index += 1;
            }
            'A' | 'a' => {
                if index + 6 >= tokens.len() {
                    break;
                }
                let Some(mut x) = tokens[index + 5].parse::<f64>().ok() else {
                    break;
                };
                let Some(mut y) = tokens[index + 6].parse::<f64>().ok() else {
                    break;
                };
                if command == 'a' {
                    x += current.x_units;
                    y += current.y_units;
                }
                add_path_point(&mut points, x, y);
                current = PointUnits {
                    x_units: x,
                    y_units: y,
                };
                if start.is_none() {
                    start = Some(current);
                }
                index += 7;
            }
            'C' | 'c' => {
                if index + 5 >= tokens.len() {
                    break;
                }
                let Some(mut x) = tokens[index + 4].parse::<f64>().ok() else {
                    break;
                };
                let Some(mut y) = tokens[index + 5].parse::<f64>().ok() else {
                    break;
                };
                if command == 'c' {
                    x += current.x_units;
                    y += current.y_units;
                }
                add_path_point(&mut points, x, y);
                current = PointUnits {
                    x_units: x,
                    y_units: y,
                };
                if start.is_none() {
                    start = Some(current);
                }
                index += 6;
            }
            'Q' | 'q' | 'S' | 's' => {
                if index + 3 >= tokens.len() {
                    break;
                }
                let Some(mut x) = tokens[index + 2].parse::<f64>().ok() else {
                    break;
                };
                let Some(mut y) = tokens[index + 3].parse::<f64>().ok() else {
                    break;
                };
                if command.is_ascii_lowercase() {
                    x += current.x_units;
                    y += current.y_units;
                }
                add_path_point(&mut points, x, y);
                current = PointUnits {
                    x_units: x,
                    y_units: y,
                };
                if start.is_none() {
                    start = Some(current);
                }
                index += 4;
            }
            _ => index += 1,
        }
    }
    points
}

fn tokenize_svg_path(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_ascii_alphabetic() {
            if !current.trim().is_empty() {
                tokens.push(current.trim().to_string());
                current.clear();
            }
            tokens.push(ch.to_string());
            continue;
        }
        if ch == ',' || ch.is_whitespace() {
            if !current.trim().is_empty() {
                tokens.push(current.trim().to_string());
                current.clear();
            }
            continue;
        }
        if (ch == '-' || ch == '+') && !current.is_empty() && current != "e" && current != "E" {
            if !current.trim().is_empty() {
                tokens.push(current.trim().to_string());
                current.clear();
            }
        }
        current.push(ch);
        if matches!(chars.peek(), Some(next) if next.is_ascii_alphabetic() || *next == ',' || next.is_whitespace())
        {
            if !current.trim().is_empty() {
                tokens.push(current.trim().to_string());
                current.clear();
            }
        }
    }
    if !current.trim().is_empty() {
        tokens.push(current.trim().to_string());
    }
    tokens
}

fn add_path_point(points: &mut Vec<PointUnits>, x_units: f64, y_units: f64) {
    let point = PointUnits { x_units, y_units };
    if points
        .last()
        .copied()
        .is_some_and(|last| same_point(last, point))
    {
        return;
    }
    points.push(point);
}
fn same_point(left: PointUnits, right: PointUnits) -> bool {
    (left.x_units - right.x_units).abs() < 1e-9 && (left.y_units - right.y_units).abs() < 1e-9
}

pub fn write_schlib(component: &Component, output_path: &Path) -> Result<()> {
    let file = File::create(output_path)?;
    let mut compound = cfb::CompoundFile::create(file)?;
    let section_key = old::section_key_from_name(&component.name);
    write_stream(&mut compound, "/FileHeader", &file_header_bytes(component))?;
    if section_key != component.name {
        write_stream(
            &mut compound,
            "/SectionKeys",
            &section_keys_bytes(&component.name, &section_key),
        )?;
    }
    compound.create_storage(&format!("/{section_key}/"))?;
    write_stream(
        &mut compound,
        &format!("/{section_key}/Data"),
        &component_data_bytes(component),
    )?;
    write_stream(&mut compound, "/Storage", &storage_bytes())?;
    compound.flush()?;
    Ok(())
}

fn write_stream(
    compound: &mut cfb::CompoundFile<File>,
    stream_path: &str,
    data: &[u8],
) -> std::io::Result<()> {
    let mut stream = compound.create_stream(stream_path)?;
    stream.write_all(data)
}

fn file_header_bytes(component: &Component) -> Vec<u8> {
    let mut writer = old::BinaryWriter::default();
    let mut params = old::Params::default();
    params.push(
        "HEADER",
        "Protel for Windows - Schematic Library Editor Binary File Version 5.0",
    );
    params.push("WEIGHT", schlib_weight(component).to_string());
    params.push("MINORVERSION", "2");
    params.push("FONTIDCOUNT", "1");
    params.push("SIZE1", "10");
    params.push("FONTNAME1", "Times New Roman");
    params.push("USEMBCS", "T");
    params.push("ISBOC", "T");
    params.push("SHEETSTYLE", "9");
    params.push("SYSTEMFONT", "1");
    params.push("BORDERON", "T");
    params.push("SHEETNUMBERSPACESIZE", "12");
    params.push("AREACOLOR", "16317695");
    params.push("SNAPGRIDON", "T");
    params.push("SNAPGRIDSIZE", "10");
    params.push("VISIBLEGRIDON", "T");
    params.push("VISIBLEGRIDSIZE", "10");
    params.push("CUSTOMX", "18000");
    params.push("CUSTOMY", "18000");
    params.push("USECUSTOMSHEET", "T");
    params.push("REFERENCEZONESON", "T");
    params.push("DISPLAY_UNIT", "0");
    params.push("COMPCOUNT", "1");
    params.push("LIBREF0", &component.name);
    params.push("COMPDESCR0", &component.description);
    params.push("PARTCOUNT0", (component.part_count + 1).to_string());
    writer.write_cstring_param_block(&params);
    writer.write_i32(1);
    writer.write_string_block(&component.name);
    writer.into_inner()
}

fn section_keys_bytes(component_name: &str, section_key: &str) -> Vec<u8> {
    let mut writer = old::BinaryWriter::default();
    let mut params = old::Params::default();
    params.push("KeyCount", "1");
    params.push("LibRef0", component_name);
    params.push("SectionKey0", section_key);
    writer.write_cstring_param_block(&params);
    writer.into_inner()
}
fn storage_bytes() -> Vec<u8> {
    let mut writer = old::BinaryWriter::default();
    let mut params = old::Params::default();
    params.push("HEADER", "Icon storage");
    writer.write_cstring_param_block(&params);
    writer.into_inner()
}
fn schlib_weight(component: &Component) -> usize {
    1 + component.rectangles.len()
        + component.labels.len()
        + component.polylines.len()
        + component.arcs.len()
        + component.ellipses.len()
        + component.pins.len()
        + 3
}

fn component_data_bytes(component: &Component) -> Vec<u8> {
    let mut writer = old::BinaryWriter::default();
    let mut params = old::Params::default();
    params.push("RECORD", "1");
    params.push("LIBREFERENCE", &component.name);
    params.push("COMPONENTDESCRIPTION", &component.description);
    params.push("PARTCOUNT", (component.part_count + 1).to_string());
    params.push("DISPLAYMODECOUNT", "1");
    params.push("OWNERPARTID", "-1");
    params.push("CURRENTPARTID", "1");
    params.push("LIBRARYPATH", "*");
    params.push("SOURCELIBRARYNAME", "*");
    params.push("SHEETPARTFILENAME", "*");
    params.push("TARGETFILENAME", "*");
    params.push("UNIQUEID", stable_unique_id(&component.name, "COMP"));
    params.push("AREACOLOR", "11599871");
    params.push("COLOR", "128");
    params.push("PARTIDLOCKED", "T");
    params.push("DESIGNITEMID", &component.name);
    if !component.pins.is_empty() {
        params.push("ALLPINCOUNT", component.pins.len().to_string());
    }
    writer.write_cstring_param_block(&params);
    for (index, rect) in component.rectangles.iter().enumerate() {
        let mut p = old::Params::default();
        p.push("RECORD", "14");
        push_owned_part(&mut p, rect.owner_part_id);
        p.push_coord("LOCATION.X", rect.corner1.x_raw);
        p.push_coord("LOCATION.Y", rect.corner1.y_raw);
        p.push_coord("CORNER.X", rect.corner2.x_raw);
        p.push_coord("CORNER.Y", rect.corner2.y_raw);
        p.push("LINEWIDTH", rect.line_width_index.to_string());
        p.push_non_zero("COLOR", rect.color_bgr);
        p.push("AREACOLOR", rect.fill_color_bgr.to_string());
        p.push_bool("ISSOLID", rect.is_filled);
        p.push_bool("TRANSPARENT", rect.is_transparent);
        p.push(
            "UNIQUEID",
            stable_unique_id(
                &component.name,
                &format!("RECT{}_{index}", rect.owner_part_id),
            ),
        );
        writer.write_cstring_param_block(&p);
    }
    for (index, label) in component.labels.iter().enumerate() {
        let mut p = old::Params::default();
        p.push("RECORD", "4");
        push_owned_part(&mut p, label.owner_part_id);
        p.push_coord("LOCATION.X", label.location.x_raw);
        p.push_coord("LOCATION.Y", label.location.y_raw);
        p.push("FONTID", "1");
        p.push("TEXT", &label.text);
        p.push_non_zero("COLOR", label.color_bgr);
        p.push_non_zero("ORIENTATION", label.orientation as i32);
        p.push(
            "UNIQUEID",
            stable_unique_id(
                &component.name,
                &format!("TEXT{}_{index}", label.owner_part_id),
            ),
        );
        writer.write_cstring_param_block(&p);
    }
    for (index, polyline) in component.polylines.iter().enumerate() {
        let mut p = old::Params::default();
        p.push("RECORD", "6");
        push_owned_part(&mut p, polyline.owner_part_id);
        p.push("LINEWIDTH", polyline.line_width_index.to_string());
        p.push_non_zero("COLOR", polyline.color_bgr);
        p.push("LOCATIONCOUNT", polyline.points.len().to_string());
        for (point_index, point) in polyline.points.iter().enumerate() {
            p.push_coord(&format!("X{}", point_index + 1), point.x_raw);
            p.push_coord(&format!("Y{}", point_index + 1), point.y_raw);
        }
        p.push(
            "UNIQUEID",
            stable_unique_id(
                &component.name,
                &format!("POLY{}_{index}", polyline.owner_part_id),
            ),
        );
        writer.write_cstring_param_block(&p);
    }
    for (index, arc) in component.arcs.iter().enumerate() {
        let mut p = old::Params::default();
        p.push("RECORD", "12");
        push_owned_part(&mut p, arc.owner_part_id);
        p.push_coord("LOCATION.X", arc.center.x_raw);
        p.push_coord("LOCATION.Y", arc.center.y_raw);
        p.push_coord("RADIUS", arc.radius_raw);
        p.push("LINEWIDTH", arc.line_width_index.to_string());
        if arc.start_angle.abs() > f64::EPSILON {
            p.push("STARTANGLE", format_angle(arc.start_angle));
        }
        p.push("ENDANGLE", format_angle(arc.end_angle));
        p.push_non_zero("COLOR", arc.color_bgr);
        p.push(
            "UNIQUEID",
            stable_unique_id(
                &component.name,
                &format!("ARC{}_{index}", arc.owner_part_id),
            ),
        );
        writer.write_cstring_param_block(&p);
    }
    for (index, ellipse) in component.ellipses.iter().enumerate() {
        let mut p = old::Params::default();
        p.push("RECORD", "8");
        push_owned_part(&mut p, ellipse.owner_part_id);
        p.push_coord("LOCATION.X", ellipse.center.x_raw);
        p.push_coord("LOCATION.Y", ellipse.center.y_raw);
        p.push_coord("RADIUS", ellipse.radius_x_raw);
        p.push_coord("SECONDARYRADIUS", ellipse.radius_y_raw);
        p.push("LINEWIDTH", ellipse.line_width_index.to_string());
        p.push_non_zero("COLOR", ellipse.color_bgr);
        p.push("AREACOLOR", ellipse.fill_color_bgr.to_string());
        p.push_bool("ISSOLID", ellipse.is_filled);
        p.push_bool("TRANSPARENT", ellipse.is_transparent);
        p.push(
            "UNIQUEID",
            stable_unique_id(
                &component.name,
                &format!("ELLIPSE{}_{index}", ellipse.owner_part_id),
            ),
        );
        writer.write_cstring_param_block(&p);
    }
    for pin in &component.pins {
        writer.write_block(0x01, |w| {
            w.write_i32(2);
            w.write_u8(0);
            w.write_i16(pin.owner_part_id.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
            w.write_u8(pin.owner_part_display_mode as u8);
            w.write_u8(0);
            w.write_u8(0);
            w.write_u8(0);
            w.write_u8(0);
            w.write_pascal_short_string("");
            w.write_u8(0);
            w.write_u8(4);
            w.write_u8(pin_conglomerate(pin));
            w.write_i16(old::dxp_i16(pin.length_raw));
            w.write_i16(old::dxp_i16(pin.location.x_raw));
            w.write_i16(old::dxp_i16(pin.location.y_raw));
            w.write_i32(pin.color_bgr);
            w.write_pascal_short_string(&pin.name);
            w.write_pascal_short_string(&pin.designator);
            w.write_pascal_short_string("");
            w.write_pascal_short_string("");
            w.write_pascal_short_string("");
        });
    }
    let mut d = old::Params::default();
    d.push("RECORD", "34");
    d.push("OWNERPARTID", "-1");
    d.push("LOCATION.X_FRAC", "-5");
    d.push("LOCATION.Y_FRAC", "5");
    d.push("COLOR", "8388608");
    d.push("FONTID", "1");
    d.push("TEXT", "*");
    d.push("NAME", "Designator");
    d.push("READONLYSTATE", "1");
    d.push("UNIQUEID", stable_unique_id(&component.name, "DESIGNATOR"));
    writer.write_cstring_param_block(&d);
    let mut c = old::Params::default();
    c.push("RECORD", "41");
    c.push("OWNERPARTID", "-1");
    c.push("LOCATION.X_FRAC", "-5");
    c.push("LOCATION.Y_FRAC", "-15");
    c.push("COLOR", "8388608");
    c.push("FONTID", "1");
    c.push("ISHIDDEN", "T");
    c.push("TEXT", "*");
    c.push("NAME", "Comment");
    c.push("UNIQUEID", stable_unique_id(&component.name, "COMMENT"));
    writer.write_cstring_param_block(&c);
    let mut f = old::Params::default();
    f.push("RECORD", "44");
    writer.write_cstring_param_block(&f);
    writer.into_inner()
}

fn push_owned_part(params: &mut old::Params, owner_part_id: i32) {
    params.push_bool("ISNOTACCESIBLE", true);
    params.push("OWNERPARTID", owner_part_id.to_string());
}
fn format_angle(angle: f64) -> String {
    format!("{:.3}", old::normalize_angle(angle))
}
fn text_orientation_from_rotation(rotation: f64) -> u8 {
    ((old::normalize_angle(rotation) / 90.0).round() as i32).rem_euclid(4) as u8
}
fn pin_orientation_from_easyeda_rotation(rotation: f64) -> u8 {
    ((old::normalize_angle(rotation) / 90.0).round() as i32).rem_euclid(4) as u8
}
fn pin_inner_location_from_easyeda(
    x_units: f64,
    y_units: f64,
    length_units: f64,
    rotation: f64,
) -> (f64, f64) {
    let angle = old::normalize_angle(rotation);
    let (dx_units, dy_units) = if !(45.0..315.0).contains(&angle) {
        (length_units, 0.0)
    } else if angle < 135.0 {
        (0.0, length_units)
    } else if angle < 225.0 {
        (-length_units, 0.0)
    } else {
        (0.0, -length_units)
    };
    (x_units + dx_units, y_units + dy_units)
}

fn arc_from_raw(raw: &ArcRaw) -> Option<Arc> {
    let (x1, y1, x2, y2, x3, y3) = (
        raw.start.x_units,
        raw.start.y_units,
        raw.mid.x_units,
        raw.mid.y_units,
        raw.end.x_units,
        raw.end.y_units,
    );
    let divisor = 2.0 * (x1 * (y2 - y3) + x2 * (y3 - y1) + x3 * (y1 - y2));
    if divisor.abs() <= 1e-9 {
        return None;
    }
    let x1_sq = x1 * x1 + y1 * y1;
    let x2_sq = x2 * x2 + y2 * y2;
    let x3_sq = x3 * x3 + y3 * y3;
    let cx = (x1_sq * (y2 - y3) + x2_sq * (y3 - y1) + x3_sq * (y1 - y2)) / divisor;
    let cy = (x1_sq * (x3 - x2) + x2_sq * (x1 - x3) + x3_sq * (x2 - x1)) / divisor;
    let radius = ((x1 - cx).powi(2) + (y1 - cy).powi(2)).sqrt();
    if !radius.is_finite() || radius <= 1e-9 {
        return None;
    }
    let mut start_angle = point_angle_degrees(cx, cy, x1, y1);
    let mid_angle = point_angle_degrees(cx, cy, x2, y2);
    let mut end_angle = point_angle_degrees(cx, cy, x3, y3);
    if !angle_lies_on_ccw_path(start_angle, mid_angle, end_angle) {
        std::mem::swap(&mut start_angle, &mut end_angle);
    }
    Some(Arc {
        center: old::CoordPoint::from_symbol_units(cx, cy),
        radius_raw: old::raw_from_symbol_units(radius),
        start_angle,
        end_angle,
        color_bgr: old::RED_BGR,
        line_width_index: GRAPHIC_LINE_WIDTH_INDEX,
        owner_part_id: raw.owner_part_id,
    })
}

fn point_angle_degrees(cx: f64, cy: f64, x: f64, y: f64) -> f64 {
    old::normalize_angle((y - cy).atan2(x - cx).to_degrees())
}
fn angle_lies_on_ccw_path(start: f64, mid: f64, end: f64) -> bool {
    angle_delta_ccw(start, mid) <= angle_delta_ccw(start, end) + 1e-6
}
fn angle_delta_ccw(start: f64, end: f64) -> f64 {
    let mut delta = old::normalize_angle(end) - old::normalize_angle(start);
    if delta < 0.0 {
        delta += 360.0;
    }
    delta
}
fn stable_unique_id(name: &str, salt: &str) -> String {
    old::stable_unique_id(name, salt)
}
fn normalize_component_name(name: &str) -> String {
    old::normalize_component_name(name)
}
fn pin_conglomerate(pin: &Pin) -> u8 {
    let mut flags = pin.orientation & 0x03;
    if pin.show_name {
        flags |= 0x08;
    }
    if pin.show_designator {
        flags |= 0x10;
    }
    flags
}

#[derive(Debug, Clone, Copy)]
struct PointUnits {
    x_units: f64,
    y_units: f64,
}
#[derive(Debug, Clone)]
struct PartRaw {
    owner_part_id: i32,
    bounds: old::OptionalBounds,
    pins: Vec<PinRaw>,
    rectangles: Vec<RectRaw>,
    polylines: Vec<PolylineRaw>,
    arcs: Vec<ArcRaw>,
    ellipses: Vec<EllipseRaw>,
    labels: Vec<TextRaw>,
}
impl PartRaw {
    fn new(owner_part_id: i32, declared_bounds: Option<old::Bounds>) -> Self {
        let mut bounds = old::OptionalBounds::default();
        if let Some(bounds_decl) = declared_bounds {
            bounds.update_x(bounds_decl.min_x_units, bounds_decl.max_x_units);
            bounds.update_y(bounds_decl.min_y_units, bounds_decl.max_y_units);
        }
        Self {
            owner_part_id,
            bounds,
            pins: Vec::new(),
            rectangles: Vec::new(),
            polylines: Vec::new(),
            arcs: Vec::new(),
            ellipses: Vec::new(),
            labels: Vec::new(),
        }
    }
    fn has_graphics(&self) -> bool {
        !self.rectangles.is_empty()
            || !self.polylines.is_empty()
            || !self.arcs.is_empty()
            || !self.ellipses.is_empty()
            || !self.labels.is_empty()
    }
    fn has_complex_body(&self) -> bool {
        !self.polylines.is_empty() || !self.arcs.is_empty()
    }
}
#[derive(Debug, Clone)]
struct PinRaw {
    id: String,
    x_units: f64,
    y_units: f64,
    length_units: f64,
    rotation_degrees: f64,
    owner_part_id: i32,
}
#[derive(Debug, Clone, Copy)]
struct RectRaw {
    x1_units: f64,
    y1_units: f64,
    x2_units: f64,
    y2_units: f64,
    owner_part_id: i32,
}
#[derive(Debug, Clone)]
struct PolylineRaw {
    points: Vec<PointUnits>,
    owner_part_id: i32,
}
#[derive(Debug, Clone, Copy)]
struct ArcRaw {
    start: PointUnits,
    mid: PointUnits,
    end: PointUnits,
    owner_part_id: i32,
}
#[derive(Debug, Clone, Copy)]
struct EllipseRaw {
    center_x_units: f64,
    center_y_units: f64,
    radius_x_units: f64,
    radius_y_units: f64,
    owner_part_id: i32,
    is_filled: bool,
    is_transparent: bool,
}
#[derive(Debug, Clone)]
struct TextRaw {
    text: String,
    x_units: f64,
    y_units: f64,
    rotation_degrees: f64,
    owner_part_id: i32,
}
#[derive(Debug)]
struct Pin {
    designator: String,
    name: String,
    location: old::CoordPoint,
    length_raw: i64,
    orientation: u8,
    show_name: bool,
    show_designator: bool,
    color_bgr: i32,
    owner_part_id: i32,
    owner_part_display_mode: i32,
}
#[derive(Debug)]
struct Rectangle {
    corner1: old::CoordPoint,
    corner2: old::CoordPoint,
    color_bgr: i32,
    fill_color_bgr: i32,
    is_filled: bool,
    is_transparent: bool,
    line_width_index: i32,
    owner_part_id: i32,
}
#[derive(Debug)]
struct Polyline {
    points: Vec<old::CoordPoint>,
    color_bgr: i32,
    line_width_index: i32,
    owner_part_id: i32,
}
#[derive(Debug)]
struct Arc {
    center: old::CoordPoint,
    radius_raw: i64,
    start_angle: f64,
    end_angle: f64,
    color_bgr: i32,
    line_width_index: i32,
    owner_part_id: i32,
}
#[derive(Debug)]
struct Ellipse {
    center: old::CoordPoint,
    radius_x_raw: i64,
    radius_y_raw: i64,
    color_bgr: i32,
    fill_color_bgr: i32,
    is_filled: bool,
    is_transparent: bool,
    line_width_index: i32,
    owner_part_id: i32,
}
#[derive(Debug)]
struct Label {
    text: String,
    location: old::CoordPoint,
    orientation: u8,
    color_bgr: i32,
    owner_part_id: i32,
}
#[derive(Debug)]
pub struct Component {
    name: String,
    description: String,
    part_count: usize,
    pins: Vec<Pin>,
    rectangles: Vec<Rectangle>,
    polylines: Vec<Polyline>,
    arcs: Vec<Arc>,
    ellipses: Vec<Ellipse>,
    labels: Vec<Label>,
}
