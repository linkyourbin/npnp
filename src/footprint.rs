use serde_json::Value;

use crate::error::{AppError, Result};
use crate::pcblib::{
    CoordPoint, LAYER_BOTTOM, LAYER_BOTTOM_OVERLAY, LAYER_MECHANICAL_1, LAYER_MECHANICAL_2,
    LAYER_MECHANICAL_5, LAYER_MECHANICAL_6, LAYER_MECHANICAL_9, LAYER_MULTI, LAYER_TOP,
    LAYER_TOP_OVERLAY, PAD_HOLE_ROUND, PAD_HOLE_SLOT, PAD_HOLE_SQUARE, PAD_SHAPE_OCTAGONAL,
    PAD_SHAPE_RECTANGULAR, PAD_SHAPE_ROUND, PAD_SHAPE_ROUNDED_RECTANGLE, PcbArc, PcbComponent,
    PcbComponentBody, PcbExtendedPrimitiveInfo, PcbLibrary, PcbModel, PcbPad, PcbRegion, PcbTrack,
    stable_guid,
};
use crate::util::{nested_string, sanitize_filename};

const FOOTPRINT_UNIT_TO_MM: f64 = 0.0254;
const RAW_PER_MIL: f64 = 10_000.0;
const DEFAULT_GRAPHIC_WIDTH_MM: f64 = 0.05;
const CIRCLE_SEGMENTS: usize = 32;
const DEFAULT_CORNER_RADIUS_PERCENTAGE: u8 = 50;
const MIN_COMPONENT_BODY_HEIGHT_MM: f64 = 0.2;
const CUSTOM_PAD_HOTSPOT_UNITS: f64 = 2.3792;
const DEFAULT_PAD_SOLDER_MASK_EXPANSION_MIL: f64 = 1.969;

pub fn build_pcblib_from_payload(
    payload: &Value,
    component_name: &str,
    step_bytes: Option<&[u8]>,
) -> Result<PcbLibrary> {
    let rows = parse_easyeda_rows(payload)?;
    let model_3d = parse_footprint_3d_model(payload);
    let mut pads = Vec::new();
    let mut overlay_polys = Vec::new();
    let mut overlay_circles = Vec::new();
    let mut overlay_arcs = Vec::new();
    let mut overlay_regions = Vec::new();
    let mut bounds = Bounds::default();
    let mut body_bounds = Bounds::default();
    let mut fallback_designator = 1usize;

    for row in &rows {
        let Some(row_type) = row.first().and_then(Value::as_str) else {
            continue;
        };
        match row_type.trim().to_ascii_uppercase().as_str() {
            "PAD" => {
                let layer_code = row_i32(row, 4, 1);
                let mut designator = row_string(row, 5);
                if designator.trim().is_empty() {
                    designator = fallback_designator.to_string();
                    fallback_designator += 1;
                }
                let x = row_f64(row, 6, 0.0);
                let y = row_f64(row, 7, 0.0);
                let mut rotation = row_f64(row, 8, f64::NAN);
                if rotation.is_nan() {
                    rotation = row_f64(row, 14, 0.0);
                }
                let mut hole = row_f64(row, 9, 0.0);
                let mut hole_slot = hole;
                let mut hole_shape = "ROUND".to_string();
                if let Some(Value::Array(hole_array)) = row.get(9) {
                    hole_shape =
                        value_string(hole_array.first()).unwrap_or_else(|| "ROUND".to_string());
                    hole = value_f64(hole_array.get(1)).unwrap_or(0.0);
                    hole_slot = value_f64(hole_array.get(2)).unwrap_or(hole);
                }
                let mut width: f64 = 10.0;
                let mut height: f64 = 10.0;
                let mut shape = "ROUND".to_string();
                let mut polygon_points = None;
                if let Some(Value::Array(shape_array)) = row.get(10) {
                    shape =
                        value_string(shape_array.first()).unwrap_or_else(|| "ROUND".to_string());
                    if shape.eq_ignore_ascii_case("POLY") {
                        if let Some(poly_shape) = shape_array.get(1) {
                            let poly_raw_points = parse_path_raw_points(poly_shape);
                            if let Some(poly_bounds) = Bounds::from_raw_points(&poly_raw_points) {
                                width = width.max(poly_bounds.max_x - poly_bounds.min_x);
                                height = height.max(poly_bounds.max_y - poly_bounds.min_y);
                                if poly_raw_points.len() >= 3 {
                                    polygon_points = Some(poly_raw_points);
                                }
                            }
                        }
                    } else {
                        width = value_f64(shape_array.get(1)).unwrap_or(width);
                        height = value_f64(shape_array.get(2)).unwrap_or(width);
                    }
                }
                if width <= 0.0 {
                    width = 10.0;
                }
                if height <= 0.0 {
                    height = width;
                }
                pads.push(PadRaw {
                    designator,
                    x,
                    y,
                    width,
                    height,
                    hole: hole.max(0.0),
                    hole_slot: hole_slot.max(hole),
                    hole_shape,
                    rotation,
                    layer_code,
                    shape,
                    polygon_points,
                    custom_mask_expansion_units: value_f64(row.get(17))
                        .filter(|value| *value > 0.0)
                        .or_else(|| value_f64(row.get(18)).filter(|value| *value > 0.0)),
                });
                bounds.update_span(
                    x - width / 2.0,
                    x + width / 2.0,
                    y - height / 2.0,
                    y + height / 2.0,
                );
            }
            "POLY" => {
                let layer_code = row_i32(row, 4, -1);
                let stroke = row_f64(row, 5, 6.0);
                let Some(shape_value) = row.get(6) else {
                    continue;
                };
                if let Some(circle) = try_parse_circle_shape(shape_value) {
                    if is_component_body_layer(layer_code) {
                        body_bounds.update_span(
                            circle.cx - circle.radius,
                            circle.cx + circle.radius,
                            circle.cy - circle.radius,
                            circle.cy + circle.radius,
                        );
                        continue;
                    }
                    if !is_overlay_layer(layer_code) {
                        continue;
                    }
                    bounds.update_span(
                        circle.cx - circle.radius,
                        circle.cx + circle.radius,
                        circle.cy - circle.radius,
                        circle.cy + circle.radius,
                    );
                    overlay_circles.push(CircleRaw {
                        layer_code,
                        width: stroke,
                        cx: circle.cx,
                        cy: circle.cy,
                        radius: circle.radius,
                    });
                    continue;
                }
                let raw_points = parse_path_raw_points(shape_value);
                if raw_points.len() < 2 {
                    continue;
                }
                if is_component_body_layer(layer_code) {
                    body_bounds.update_from_raw_points(&raw_points);
                    continue;
                }
                if !is_overlay_layer(layer_code) {
                    continue;
                }
                bounds.update_from_raw_points(&raw_points);
                overlay_polys.push(PolyRaw {
                    layer_code,
                    width: stroke,
                    points: raw_points.into_iter().map(raw_point_to_coord).collect(),
                });
            }
            "FILL" => {
                let layer_code = row_i32(row, 4, -1);
                let Some(Value::Array(shapes)) = row.get(7) else {
                    continue;
                };
                if is_component_body_layer(layer_code) {
                    for shape in shapes {
                        if let Some(circle) = try_parse_circle_shape(shape) {
                            body_bounds.update_span(
                                circle.cx - circle.radius,
                                circle.cx + circle.radius,
                                circle.cy - circle.radius,
                                circle.cy + circle.radius,
                            );
                            continue;
                        }
                        let raw_points = parse_path_raw_points(shape);
                        if raw_points.len() < 3 {
                            continue;
                        }
                        body_bounds.update_from_raw_points(&raw_points);
                    }
                    continue;
                }
                if !is_overlay_layer(layer_code) {
                    continue;
                }
                for shape in shapes {
                    if let Some(circle) = try_parse_circle_shape(shape) {
                        bounds.update_span(
                            circle.cx - circle.radius,
                            circle.cx + circle.radius,
                            circle.cy - circle.radius,
                            circle.cy + circle.radius,
                        );
                        overlay_regions.push(RegionRaw {
                            layer_code,
                            points: circle_region(circle.cx, circle.cy, circle.radius),
                        });
                        continue;
                    }
                    let raw_points = parse_path_raw_points(shape);
                    if raw_points.len() < 3 {
                        continue;
                    }
                    bounds.update_from_raw_points(&raw_points);
                    let mut points: Vec<CoordPoint> =
                        raw_points.into_iter().map(raw_point_to_coord).collect();
                    if points.first() != points.last() {
                        if let Some(first) = points.first().copied() {
                            points.push(first);
                        }
                    }
                    overlay_regions.push(RegionRaw { layer_code, points });
                }
            }
            "TRACK" => {
                let layer_code = row_i32(row, 4, -1);
                let stroke = row_f64(row, 5, 6.0);
                let x1 = row_f64(row, 6, 0.0);
                let y1 = row_f64(row, 7, 0.0);
                let x2 = row_f64(row, 8, 0.0);
                let y2 = row_f64(row, 9, 0.0);
                if is_component_body_layer(layer_code) {
                    body_bounds.update_span(x1, x2, y1, y2);
                    continue;
                }
                if !is_overlay_layer(layer_code) {
                    continue;
                }
                bounds.update_span(x1, x2, y1, y2);
                overlay_polys.push(PolyRaw {
                    layer_code,
                    width: stroke,
                    points: vec![coord_from_easy_units(x1, y1), coord_from_easy_units(x2, y2)],
                });
            }
            "RECT" => {
                let layer_code = row_i32(row, 4, -1);
                let stroke = row_f64(row, 5, 6.0);
                let x1 = row_f64(row, 6, 0.0);
                let y1 = row_f64(row, 7, 0.0);
                let x2 = row_f64(row, 8, x1);
                let y2 = row_f64(row, 9, y1);
                if is_component_body_layer(layer_code) {
                    body_bounds.update_span(x1, x2, y1, y2);
                    continue;
                }
                if !is_overlay_layer(layer_code) {
                    continue;
                }
                bounds.update_span(x1, x2, y1, y2);
                overlay_polys.push(PolyRaw {
                    layer_code,
                    width: stroke,
                    points: vec![
                        coord_from_easy_units(x1, y1),
                        coord_from_easy_units(x2, y1),
                        coord_from_easy_units(x2, y2),
                        coord_from_easy_units(x1, y2),
                        coord_from_easy_units(x1, y1),
                    ],
                });
            }
            "CIRCLE" => {
                let layer_code = row_i32(row, 4, -1);
                let stroke = row_f64(row, 5, 6.0);
                let x = row_f64(row, 6, 0.0);
                let y = row_f64(row, 7, 0.0);
                let radius = row_f64(row, 8, 0.0).abs();
                if radius <= 0.000_001 {
                    continue;
                }
                if is_component_body_layer(layer_code) {
                    body_bounds.update_span(x - radius, x + radius, y - radius, y + radius);
                    continue;
                }
                if !is_overlay_layer(layer_code) {
                    continue;
                }
                bounds.update_span(x - radius, x + radius, y - radius, y + radius);
                overlay_circles.push(CircleRaw {
                    layer_code,
                    width: stroke,
                    cx: x,
                    cy: y,
                    radius,
                });
            }
            "ARC" => {
                let layer_code = row_i32(row, 4, -1);
                let stroke = row_f64(row, 5, 6.0);
                let x = row_f64(row, 6, 0.0);
                let y = row_f64(row, 7, 0.0);
                let radius = row_f64(row, 8, 0.0).abs();
                if radius <= 0.000_001 {
                    continue;
                }
                if is_component_body_layer(layer_code) {
                    body_bounds.update_span(x - radius, x + radius, y - radius, y + radius);
                    continue;
                }
                if !is_overlay_layer(layer_code) {
                    continue;
                }
                bounds.update_span(x - radius, x + radius, y - radius, y + radius);
                overlay_arcs.push(ArcRaw {
                    layer_code,
                    width: stroke,
                    cx: x,
                    cy: y,
                    radius,
                    start_angle: normalize_angle(row_f64(row, 9, 0.0)),
                    end_angle: normalize_angle(row_f64(row, 10, 0.0)),
                });
            }
            _ => {}
        }
    }

    let component_height_mm =
        resolve_component_height_mm(payload, component_name, model_3d.as_ref());
    let mut component = PcbComponent {
        name: component_name.to_string(),
        description: resolve_footprint_description(payload, component_name),
        height_raw: raw_from_mm(component_height_mm),
        pads: Vec::new(),
        arcs: Vec::new(),
        tracks: Vec::new(),
        regions: Vec::new(),
        bodies: Vec::new(),
        extended_primitive_information: Vec::new(),
    };

    let mut pad_shape_regions = Vec::new();
    let mut custom_mask_regions = Vec::new();
    let mut custom_mask_expansions = Vec::new();

    for pad_raw in pads {
        let hole_mm = easy_units_to_mm(pad_raw.hole);
        let layer = map_pad_layer(pad_raw.layer_code, hole_mm);
        let hole_type = map_pad_hole_type(&pad_raw.hole_shape);
        let hole_slot_length_raw = raw_from_mm(easy_units_to_mm(pad_raw.hole_slot));
        let is_custom_poly = hole_mm <= 0.000_001
            && pad_raw.shape.eq_ignore_ascii_case("POLY")
            && pad_raw.polygon_points.is_some();
        let (shape, width, height, rotation) = if is_custom_poly {
            (
                PAD_SHAPE_ROUND,
                CUSTOM_PAD_HOTSPOT_UNITS,
                CUSTOM_PAD_HOTSPOT_UNITS,
                0.0,
            )
        } else {
            (
                map_pad_shape(&pad_raw.shape, pad_raw.width, pad_raw.height),
                pad_raw.width,
                pad_raw.height,
                normalize_angle(pad_raw.rotation),
            )
        };
        component.pads.push(PcbPad {
            designator: pad_raw.designator.clone(),
            location: coord_from_easy_units(pad_raw.x, pad_raw.y),
            size_top: coord_from_easy_units(width, height),
            size_middle: coord_from_easy_units(width, height),
            size_bottom: coord_from_easy_units(width, height),
            hole_size_raw: raw_from_mm(hole_mm),
            shape_top: shape,
            shape_middle: shape,
            shape_bottom: shape,
            rotation,
            is_plated: true,
            layer,
            is_locked: false,
            is_tenting_top: false,
            is_tenting_bottom: false,
            is_keepout: false,
            mode: 0,
            power_plane_connect_style: 0,
            relief_air_gap_raw: 0,
            relief_conductor_width_raw: raw_from_mils(10.0),
            relief_entries: 4,
            power_plane_clearance_raw: raw_from_mils(10.0),
            power_plane_relief_expansion_raw: raw_from_mils(20.0),
            paste_mask_expansion_raw: 0,
            solder_mask_expansion_raw: if is_custom_poly {
                0
            } else {
                raw_from_mils(DEFAULT_PAD_SOLDER_MASK_EXPANSION_MIL)
            },
            drill_type: 0,
            jumper_id: 0,
            hole_type,
            hole_slot_length_raw,
            hole_rotation: 0.0,
            corner_radius_percentage: DEFAULT_CORNER_RADIUS_PERCENTAGE,
        });

        if let Some(region_outline) = pad_outline_region(&pad_raw) {
            pad_shape_regions.push(PcbRegion {
                layer: LAYER_MECHANICAL_9,
                outline: region_outline.clone(),
                kind: 0,
                net: None,
                unique_id: None,
                name: Some(" ".to_string()),
                is_locked: false,
                is_tenting_top: false,
                is_tenting_bottom: false,
                is_keepout: false,
                additional_params: pad_shape_region_params("MECHANICAL9"),
            });
            let mask_expansion = if is_custom_poly {
                pad_raw
                    .custom_mask_expansion_units
                    .unwrap_or(CUSTOM_PAD_HOTSPOT_UNITS)
            } else {
                0.0
            };
            if is_custom_poly {
                for (mask_layer, v7_layer_name) in mask_region_layers(pad_raw.layer_code, hole_mm) {
                    custom_mask_regions.push(PcbRegion {
                        layer: mask_layer,
                        outline: region_outline.clone(),
                        kind: 0,
                        net: None,
                        unique_id: None,
                        name: Some(" ".to_string()),
                        is_locked: false,
                        is_tenting_top: false,
                        is_tenting_bottom: false,
                        is_keepout: false,
                        additional_params: pad_shape_region_params(v7_layer_name),
                    });
                    custom_mask_expansions.push(mask_expansion);
                }
            }
        }
    }

    for poly in overlay_polys {
        let width_raw = resolve_graphic_width_raw(poly.width);
        for points in poly.points.windows(2) {
            component.tracks.push(PcbTrack {
                layer: map_graphic_layer(poly.layer_code),
                start: points[0],
                end: points[1],
                width_raw,
                is_locked: false,
                is_tenting_top: false,
                is_tenting_bottom: false,
                is_keepout: false,
                net_index: 0,
                component_index: 0,
            });
        }
    }
    for circle in overlay_circles {
        component.arcs.push(PcbArc {
            layer: map_graphic_layer(circle.layer_code),
            center: coord_from_easy_units(circle.cx, circle.cy),
            radius_raw: raw_from_easy_units(circle.radius),
            start_angle: 0.0,
            end_angle: 360.0,
            width_raw: resolve_graphic_width_raw(circle.width),
            is_locked: false,
            is_tenting_top: false,
            is_tenting_bottom: false,
            is_keepout: false,
        });
    }
    for arc in overlay_arcs {
        component.arcs.push(PcbArc {
            layer: map_graphic_layer(arc.layer_code),
            center: coord_from_easy_units(arc.cx, arc.cy),
            radius_raw: raw_from_easy_units(arc.radius),
            start_angle: arc.start_angle,
            end_angle: arc.end_angle,
            width_raw: resolve_graphic_width_raw(arc.width),
            is_locked: false,
            is_tenting_top: false,
            is_tenting_bottom: false,
            is_keepout: false,
        });
    }
    for region in overlay_regions {
        if region.points.len() >= 3 {
            component.regions.push(PcbRegion {
                layer: map_graphic_layer(region.layer_code),
                outline: region.points,
                kind: 0,
                net: None,
                unique_id: None,
                name: None,
                is_locked: false,
                is_tenting_top: false,
                is_tenting_bottom: false,
                is_keepout: false,
                additional_params: Vec::new(),
            });
        }
    }
    let mask_region_start =
        component.pads.len() + component.tracks.len() + component.arcs.len() + component.regions.len()
            + pad_shape_regions.len();
    component.regions.extend(pad_shape_regions);
    component.regions.extend(custom_mask_regions);
    for (offset, expansion_units) in custom_mask_expansions.into_iter().enumerate() {
        component
            .extended_primitive_information
            .push(PcbExtendedPrimitiveInfo {
                primitive_index: mask_region_start + offset,
                object_name: "Region".to_string(),
                params: vec![
                    ("TYPE".to_string(), "Mask".to_string()),
                    (
                        "SOLDERMASKEXPANSIONMODE".to_string(),
                        "Manual".to_string(),
                    ),
                    (
                        "SOLDERMASKEXPANSION_MANUAL".to_string(),
                        format!("{expansion_units:.3}mil"),
                    ),
                    (
                        "PASTEMASKEXPANSIONMODE".to_string(),
                        "None".to_string(),
                    ),
                ],
            });
    }

    let mut library = PcbLibrary::default();
    let body_extents = body_bounds.finish().or_else(|| bounds.finish());
    if let (Some(model), Some(step_data), Some(extents)) =
        (model_3d.as_ref(), step_bytes, body_extents)
    {
        let model_id = stable_guid(&format!("{}|{}", component_name, model.uri));
        let model_name = choose_step_model_name(component_name, &model.title);
        component.bodies.push(PcbComponentBody {
            layer_name: "MECHANICAL1".to_string(),
            name: "__LCEDA_BODY__".to_string(),
            kind: 0,
            subpoly_index: -1,
            union_index: 0,
            arc_resolution_raw: raw_from_mils(0.5),
            is_shape_based: false,
            cavity_height_raw: 0,
            standoff_height_raw: 0,
            overall_height_raw: raw_from_mm(component_height_mm.max(MIN_COMPONENT_BODY_HEIGHT_MM)),
            body_color_3d: 0x808080,
            body_opacity_3d: 1.0,
            body_projection: 0,
            model_id: model_id.clone(),
            model_embed: true,
            model_2d_location: CoordPoint::new(0, 0),
            model_2d_rotation: 0.0,
            model_3d_rot_x: 0.0,
            model_3d_rot_y: 0.0,
            model_3d_rot_z: 0.0,
            model_3d_dz_raw: 0,
            model_checksum: 0,
            model_name: model_name.clone(),
            model_type: 1,
            model_source: "Undefined".to_string(),
            identifier: None,
            texture: String::new(),
            outline: component_body_outline(extents),
            is_locked: false,
            is_tenting_top: false,
            is_tenting_bottom: false,
            is_keepout: false,
        });
        library.models.push(PcbModel {
            id: model_id,
            name: model_name,
            is_embedded: true,
            model_source: "Undefined".to_string(),
            rotation_x: 0.0,
            rotation_y: 0.0,
            rotation_z: 0.0,
            dz_raw: 0,
            checksum: 0,
            step_data: step_data.to_vec(),
        });
    }
    library.components.push(component);
    Ok(library)
}

fn parse_easyeda_rows(payload: &Value) -> Result<Vec<Vec<Value>>> {
    let data_str = nested_string(payload, &["result", "dataStr"])
        .or_else(|| nested_string(payload, &["dataStr"]))
        .ok_or_else(|| AppError::InvalidResponse("footprint payload has no dataStr".to_string()))?;
    let mut rows = Vec::new();
    for (line_index, line) in data_str.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let row_value: Value = serde_json::from_str(trimmed).map_err(|err| {
            AppError::InvalidResponse(format!(
                "invalid EasyEDA dataStr row {}: {err}",
                line_index + 1
            ))
        })?;
        if let Value::Array(row) = row_value {
            rows.push(row);
        }
    }
    Ok(rows)
}

fn parse_footprint_3d_model(payload: &Value) -> Option<Model3dRaw> {
    let model = payload.get("result")?.get("model_3d")?;
    let uri = model.get("uri")?.as_str()?.trim();
    if uri.is_empty() {
        return None;
    }
    let title = model
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let transform = model
        .get("transform")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut result = Model3dRaw {
        title,
        uri: uri.to_string(),
        height_mm: try_parse_height_from_model_title(
            model
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
        rotation_x: 0.0,
        rotation_y: 0.0,
        rotation_z: 0.0,
    };
    let parts: Vec<&str> = transform
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() >= 6 {
        result.rotation_x = parts[3].parse().unwrap_or(0.0);
        result.rotation_y = parts[4].parse().unwrap_or(0.0);
        result.rotation_z = parts[5].parse().unwrap_or(0.0);
    }
    Some(result)
}

fn resolve_component_height_mm(
    payload: &Value,
    component_name: &str,
    model_3d: Option<&Model3dRaw>,
) -> f64 {
    if let Some(height) = model_3d
        .and_then(|model| model.height_mm)
        .filter(|height| *height > 0.000_001)
    {
        return height;
    }

    for candidate in [
        nested_string(payload, &["result", "display_title"]),
        nested_string(payload, &["result", "title"]),
        nested_string(payload, &["result", "package"]),
        Some(component_name.to_string()),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(height) = try_parse_height_from_model_title(&candidate) {
            return height;
        }
        if let Some(height) = guess_package_family_height_mm(&candidate) {
            return height;
        }
    }

    1.0
}

fn resolve_footprint_description(payload: &Value, component_name: &str) -> String {
    let preferred = nested_string(payload, &["result", "description"])
        .or_else(|| nested_string(payload, &["description"]))
        .and_then(|text| normalize_footprint_description(&text));

    preferred
        .or_else(|| nested_string(payload, &["result", "display_title"]))
        .or_else(|| nested_string(payload, &["display_title"]))
        .or_else(|| nested_string(payload, &["result", "package"]))
        .or_else(|| nested_string(payload, &["package"]))
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| format!("Generated from EasyEDA footprint ({component_name})"))
}

fn normalize_footprint_description(text: &str) -> Option<String> {
    let parts: Vec<String> = text
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

fn try_parse_height_from_model_title(title: &str) -> Option<f64> {
    let normalized = title.trim().to_ascii_uppercase();
    let mut index = normalized.find("-H").or_else(|| normalized.find("_H"))? + 2;
    let start = index;
    let bytes = normalized.as_bytes();
    while index < bytes.len() && (bytes[index].is_ascii_digit() || bytes[index] == b'.') {
        index += 1;
    }
    (index > start)
        .then(|| normalized[start..index].parse().ok())
        .flatten()
}

fn guess_package_family_height_mm(text: &str) -> Option<f64> {
    let normalized = text.trim().to_ascii_uppercase();
    if normalized.contains("QFN") || normalized.contains("DFN") || normalized.contains("LGA") {
        Some(1.0)
    } else if normalized.contains("BGA") {
        Some(1.2)
    } else if normalized.contains("QFP")
        || normalized.contains("TQFP")
        || normalized.contains("LQFP")
    {
        Some(1.4)
    } else if normalized.contains("SOIC")
        || normalized.contains("SOP")
        || normalized.contains("SSOP")
        || normalized.contains("TSSOP")
        || normalized.contains("MSOP")
    {
        Some(1.6)
    } else if normalized.contains("SOT") {
        Some(1.6)
    } else if normalized.contains("DIP") {
        Some(4.0)
    } else {
        None
    }
}

fn component_body_outline(extents: Extents) -> Vec<CoordPoint> {
    vec![
        coord_from_easy_units(extents.min_x, extents.min_y),
        coord_from_easy_units(extents.max_x, extents.min_y),
        coord_from_easy_units(extents.max_x, extents.max_y),
        coord_from_easy_units(extents.min_x, extents.max_y),
    ]
}

fn choose_step_model_name(component_name: &str, title: &str) -> String {
    let base = if title.trim().is_empty() {
        component_name
    } else {
        title.trim()
    };
    let mut sanitized = sanitize_filename(base);
    if !sanitized.to_ascii_lowercase().ends_with(".step")
        && !sanitized.to_ascii_lowercase().ends_with(".stp")
    {
        sanitized.push_str(".step");
    }
    sanitized
}

fn is_overlay_layer(layer_code: i32) -> bool {
    matches!(layer_code, 3 | 4 | 49)
}

fn is_component_body_layer(layer_code: i32) -> bool {
    matches!(layer_code, 48 | 99)
}

fn map_graphic_layer(layer_code: i32) -> u8 {
    match layer_code {
        1 => LAYER_TOP,
        2 => LAYER_BOTTOM,
        3 => LAYER_TOP_OVERLAY,
        4 => LAYER_BOTTOM_OVERLAY,
        5 => crate::pcblib::LAYER_TOP_SOLDER,
        6 => crate::pcblib::LAYER_BOTTOM_SOLDER,
        7 => crate::pcblib::LAYER_TOP_PASTE,
        8 => crate::pcblib::LAYER_BOTTOM_PASTE,
        11 | 48 => LAYER_MECHANICAL_1,
        13 => LAYER_MECHANICAL_2,
        49 => LAYER_TOP_OVERLAY,
        50 => LAYER_MECHANICAL_5,
        51 => LAYER_MECHANICAL_6,
        12 => LAYER_MULTI,
        _ => LAYER_TOP_OVERLAY,
    }
}

fn map_pad_layer(layer_code: i32, hole_mm: f64) -> u8 {
    if layer_code == 12 || hole_mm > 0.000_001 {
        LAYER_MULTI
    } else if layer_code == 2 {
        LAYER_BOTTOM
    } else {
        LAYER_TOP
    }
}

fn map_pad_hole_type(name: &str) -> u8 {
    let upper = name.trim().to_ascii_uppercase();
    if upper.contains("SLOT") {
        PAD_HOLE_SLOT
    } else if upper.contains("SQUARE") || upper.contains("RECT") {
        PAD_HOLE_SQUARE
    } else {
        PAD_HOLE_ROUND
    }
}

fn map_pad_shape(name: &str, width: f64, height: f64) -> u8 {
    let upper = name.trim().to_ascii_uppercase();
    if upper.contains("POLY") || upper.contains("RECT") {
        PAD_SHAPE_RECTANGULAR
    } else if upper.contains("OCT") {
        PAD_SHAPE_OCTAGONAL
    } else if upper.contains("OVAL") {
        PAD_SHAPE_ROUNDED_RECTANGLE
    } else if (width - height).abs() < 0.000_001 {
        PAD_SHAPE_ROUND
    } else {
        PAD_SHAPE_ROUNDED_RECTANGLE
    }
}

fn easy_units_to_mm(value: f64) -> f64 {
    value * FOOTPRINT_UNIT_TO_MM
}
fn raw_from_easy_units(value: f64) -> i32 {
    (value * RAW_PER_MIL)
        .round()
        .clamp(i32::MIN as f64, i32::MAX as f64) as i32
}
fn raw_from_mils(value: f64) -> i32 {
    (value * RAW_PER_MIL)
        .round()
        .clamp(i32::MIN as f64, i32::MAX as f64) as i32
}
fn raw_from_mm(value: f64) -> i32 {
    raw_from_mils(value / FOOTPRINT_UNIT_TO_MM)
}
fn coord_from_easy_units(x: f64, y: f64) -> CoordPoint {
    CoordPoint::new(raw_from_easy_units(x), raw_from_easy_units(y))
}
fn raw_point_to_coord(point: RawPoint) -> CoordPoint {
    coord_from_easy_units(point.x, point.y)
}
fn resolve_graphic_width_raw(width: f64) -> i32 {
    let raw = raw_from_easy_units(width);
    if raw != 0 {
        raw
    } else {
        raw_from_mm(DEFAULT_GRAPHIC_WIDTH_MM)
    }
}
fn normalize_angle(value: f64) -> f64 {
    let mut angle = value % 360.0;
    if angle < 0.0 {
        angle += 360.0;
    }
    angle
}

fn row_f64(row: &[Value], index: usize, default: f64) -> f64 {
    value_f64(row.get(index)).unwrap_or(default)
}
fn row_i32(row: &[Value], index: usize, default: i32) -> i32 {
    value_f64(row.get(index))
        .map(|value| value.round() as i32)
        .unwrap_or(default)
}
fn row_string(row: &[Value], index: usize) -> String {
    value_string(row.get(index)).unwrap_or_default()
}
fn value_f64(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        _ => None,
    }
}
fn value_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn try_parse_circle_shape(shape: &Value) -> Option<CircleShape> {
    let array = shape.as_array()?;
    if array.len() < 4 {
        return None;
    }
    if !value_string(array.first())
        .unwrap_or_default()
        .eq_ignore_ascii_case("CIRCLE")
    {
        return None;
    }
    let radius = value_f64(array.get(3))?.abs();
    (radius > 0.000_001).then(|| CircleShape {
        cx: value_f64(array.get(1)).unwrap_or(0.0),
        cy: value_f64(array.get(2)).unwrap_or(0.0),
        radius,
    })
}

fn add_raw_point(points: &mut Vec<RawPoint>, x: f64, y: f64) {
    if let Some(last) = points.last() {
        if (last.x - x).abs() < 1e-9 && (last.y - y).abs() < 1e-9 {
            return;
        }
    }
    points.push(RawPoint { x, y });
}

fn add_axis_aligned_rect_points(
    points: &mut Vec<RawPoint>,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
) {
    let x2 = x + width;
    let y2 = y - height;
    let left = x.min(x2);
    let right = x.max(x2);
    let top = y.max(y2);
    let bottom = y.min(y2);
    let radius = radius.abs().min((right - left).abs() / 2.0).min((top - bottom).abs() / 2.0);

    if radius <= 1e-9 {
        add_raw_point(points, left, top);
        add_raw_point(points, right, top);
        add_raw_point(points, right, bottom);
        add_raw_point(points, left, bottom);
        add_raw_point(points, left, top);
        return;
    }

    const CORNER_SEGMENTS: usize = 6;
    add_raw_point(points, left + radius, top);
    add_raw_point(points, right - radius, top);
    append_corner_arc(
        points,
        right - radius,
        top - radius,
        radius,
        90.0,
        0.0,
        CORNER_SEGMENTS,
    );
    add_raw_point(points, right, bottom + radius);
    append_corner_arc(
        points,
        right - radius,
        bottom + radius,
        radius,
        0.0,
        -90.0,
        CORNER_SEGMENTS,
    );
    add_raw_point(points, left + radius, bottom);
    append_corner_arc(
        points,
        left + radius,
        bottom + radius,
        radius,
        -90.0,
        -180.0,
        CORNER_SEGMENTS,
    );
    add_raw_point(points, left, top - radius);
    append_corner_arc(
        points,
        left + radius,
        top - radius,
        radius,
        180.0,
        90.0,
        CORNER_SEGMENTS,
    );
    add_raw_point(points, left + radius, top);
}

fn append_corner_arc(
    points: &mut Vec<RawPoint>,
    center_x: f64,
    center_y: f64,
    radius: f64,
    start_degrees: f64,
    end_degrees: f64,
    segments: usize,
) {
    for step in 1..=segments {
        let t = step as f64 / segments as f64;
        let angle = (start_degrees + (end_degrees - start_degrees) * t).to_radians();
        add_raw_point(
            points,
            center_x + radius * angle.cos(),
            center_y + radius * angle.sin(),
        );
    }
}

fn parse_path_raw_points(shape: &Value) -> Vec<RawPoint> {
    let Some(array) = shape.as_array() else {
        return Vec::new();
    };
    if value_string(array.first())
        .unwrap_or_default()
        .eq_ignore_ascii_case("CIRCLE")
    {
        return Vec::new();
    }
    let mut points = Vec::new();
    let mut i = 0usize;
    while i < array.len() {
        if let Some(token) = array[i].as_str() {
            let command = token.trim().to_ascii_uppercase();
            i += 1;
            if command == "L" {
                while i + 1 < array.len() {
                    let Some(x) = value_f64(array.get(i)) else {
                        break;
                    };
                    let Some(y) = value_f64(array.get(i + 1)) else {
                        break;
                    };
                    add_raw_point(&mut points, x, y);
                    i += 2;
                }
            } else if command == "R" {
                let Some(x) = value_f64(array.get(i)) else {
                    continue;
                };
                let Some(y) = value_f64(array.get(i + 1)) else {
                    continue;
                };
                let Some(width) = value_f64(array.get(i + 2)) else {
                    continue;
                };
                let Some(height) = value_f64(array.get(i + 3)) else {
                    continue;
                };
                let radius = value_f64(array.get(i + 4)).unwrap_or(0.0);
                add_axis_aligned_rect_points(&mut points, x, y, width, height, radius);
                i += 5;
            } else if command == "ARC" || command == "A" {
                if i + 2 < array.len() && value_f64(array.get(i)).is_some() {
                    if let (Some(x), Some(y)) =
                        (value_f64(array.get(i + 1)), value_f64(array.get(i + 2)))
                    {
                        add_raw_point(&mut points, x, y);
                        i += 3;
                    }
                }
            }
            continue;
        }
        if i + 1 < array.len() {
            if let (Some(x), Some(y)) = (value_f64(array.get(i)), value_f64(array.get(i + 1))) {
                add_raw_point(&mut points, x, y);
                i += 2;
                continue;
            }
        }
        i += 1;
    }
    points
}

fn circle_region(cx: f64, cy: f64, radius: f64) -> Vec<CoordPoint> {
    (0..CIRCLE_SEGMENTS)
        .map(|index| {
            let angle = (2.0 * std::f64::consts::PI * index as f64) / CIRCLE_SEGMENTS as f64;
            coord_from_easy_units(cx + radius * angle.cos(), cy + radius * angle.sin())
        })
        .collect()
}

#[derive(Debug, Clone)]
struct PadRaw {
    designator: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    hole: f64,
    hole_slot: f64,
    hole_shape: String,
    rotation: f64,
    layer_code: i32,
    shape: String,
    polygon_points: Option<Vec<RawPoint>>,
    custom_mask_expansion_units: Option<f64>,
}
#[derive(Debug, Clone)]
struct PolyRaw {
    layer_code: i32,
    width: f64,
    points: Vec<CoordPoint>,
}
#[derive(Debug, Clone, Copy)]
struct CircleRaw {
    layer_code: i32,
    width: f64,
    cx: f64,
    cy: f64,
    radius: f64,
}
#[derive(Debug, Clone, Copy)]
struct ArcRaw {
    layer_code: i32,
    width: f64,
    cx: f64,
    cy: f64,
    radius: f64,
    start_angle: f64,
    end_angle: f64,
}
#[derive(Debug, Clone)]
struct RegionRaw {
    layer_code: i32,
    points: Vec<CoordPoint>,
}
#[derive(Debug, Clone, Copy, PartialEq)]
struct RawPoint {
    x: f64,
    y: f64,
}
#[derive(Debug, Clone, Copy)]
struct CircleShape {
    cx: f64,
    cy: f64,
    radius: f64,
}
#[derive(Debug, Clone)]
struct Model3dRaw {
    title: String,
    uri: String,
    height_mm: Option<f64>,
    rotation_x: f64,
    rotation_y: f64,
    rotation_z: f64,
}
#[derive(Debug, Clone, Copy)]
struct Extents {
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
}

#[derive(Debug, Default, Clone, Copy)]
struct Bounds {
    min_x: Option<f64>,
    max_x: Option<f64>,
    min_y: Option<f64>,
    max_y: Option<f64>,
}
impl Bounds {
    fn update_span(&mut self, x1: f64, x2: f64, y1: f64, y2: f64) {
        self.update_x(x1.min(x2), x1.max(x2));
        self.update_y(y1.min(y2), y1.max(y2));
    }
    fn update_from_raw_points(&mut self, points: &[RawPoint]) {
        for point in points {
            self.update_span(point.x, point.x, point.y, point.y);
        }
    }
    fn update_x(&mut self, min: f64, max: f64) {
        self.min_x = Some(self.min_x.map_or(min, |value| value.min(min)));
        self.max_x = Some(self.max_x.map_or(max, |value| value.max(max)));
    }
    fn update_y(&mut self, min: f64, max: f64) {
        self.min_y = Some(self.min_y.map_or(min, |value| value.min(min)));
        self.max_y = Some(self.max_y.map_or(max, |value| value.max(max)));
    }
    fn finish(self) -> Option<Extents> {
        Some(Extents {
            min_x: self.min_x?,
            max_x: self.max_x?,
            min_y: self.min_y?,
            max_y: self.max_y?,
        })
    }
    fn from_raw_points(points: &[RawPoint]) -> Option<Extents> {
        let mut bounds = Bounds::default();
        bounds.update_from_raw_points(points);
        bounds.finish()
    }
}

fn pad_outline_region(pad: &PadRaw) -> Option<Vec<CoordPoint>> {
    if let Some(points) = &pad.polygon_points {
        let mut outline: Vec<CoordPoint> = points.iter().copied().map(raw_point_to_coord).collect();
        if outline.first() != outline.last() {
            if let Some(first) = outline.first().copied() {
                outline.push(first);
            }
        }
        return (outline.len() >= 4).then_some(outline);
    }
    rectangular_pad_outline(pad.x, pad.y, pad.width, pad.height, pad.rotation)
}

fn rectangular_pad_outline(
    center_x: f64,
    center_y: f64,
    width: f64,
    height: f64,
    rotation_degrees: f64,
) -> Option<Vec<CoordPoint>> {
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let half_w = width / 2.0;
    let half_h = height / 2.0;
    let angle = normalize_angle(rotation_degrees).to_radians();
    let sin = angle.sin();
    let cos = angle.cos();
    let corners = [
        (-half_w, -half_h),
        (half_w, -half_h),
        (half_w, half_h),
        (-half_w, half_h),
    ];
    let mut outline = Vec::with_capacity(5);
    for (dx, dy) in corners {
        let x = center_x + dx * cos - dy * sin;
        let y = center_y + dx * sin + dy * cos;
        outline.push(coord_from_easy_units(x, y));
    }
    outline.push(outline[0]);
    Some(outline)
}

fn pad_shape_region_params(v7_layer: &str) -> Vec<(String, String)> {
    vec![
        ("V7_LAYER".to_string(), v7_layer.to_string()),
        ("SUBPOLYINDEX".to_string(), "-1".to_string()),
        ("UNIONINDEX".to_string(), "0".to_string()),
        ("ARCRESOLUTION".to_string(), "0.5mil".to_string()),
        ("ISSHAPEBASED".to_string(), "FALSE".to_string()),
        ("CAVITYHEIGHT".to_string(), "0mil".to_string()),
    ]
}

fn mask_region_layers(layer_code: i32, hole_mm: f64) -> Vec<(u8, &'static str)> {
    if layer_code == 2 {
        vec![(LAYER_BOTTOM, "BOTTOM")]
    } else if layer_code == 12 || hole_mm > 0.000_001 {
        vec![(LAYER_TOP, "TOP"), (LAYER_BOTTOM, "BOTTOM")]
    } else {
        vec![(LAYER_TOP, "TOP")]
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RawPoint, build_pcblib_from_payload, normalize_footprint_description, parse_path_raw_points,
        rectangular_pad_outline,
    };
    use crate::pcblib::{LAYER_MECHANICAL_9, PAD_SHAPE_ROUND};
    use serde_json::json;

    #[test]
    fn builds_pcblib_primitives_from_easyeda_footprint() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["PAD","e1",0,"",1,"1",10,20,90,null,["OVAL",30,40],[],0,0,0,1]
["TRACK","t1",0,"",3,6,0,0,10,0]
["CIRCLE","c1",0,"",3,2,5,5,2]
["FILL","f1",0,"",49,0.2,0,[[0,0,"L",10,0,10,10,0,10,0,0]],0]"#, "model_3d": {"title":"BODY-H1.2", "uri":"modeluuid", "transform":"0,0,0,1,2,3"}}});
        let library =
            build_pcblib_from_payload(&payload, "TEST", Some(b"ISO-10303-21;END-ISO-10303-21;"))
                .unwrap();
        let component = &library.components[0];
        assert_eq!(component.pads.len(), 1);
        assert_eq!(component.tracks.len(), 1);
        assert_eq!(component.arcs.len(), 1);
        assert_eq!(component.regions.len(), 2);
        assert!(component
            .regions
            .iter()
            .any(|region| region.layer == LAYER_MECHANICAL_9));
        assert_eq!(component.bodies.len(), 1);
        assert_eq!(library.models.len(), 1);
    }

    #[test]
    fn skips_body_when_step_model_is_missing() {
        let payload = json!({"result": {"display_title": "QFN-60_L7.0-W7.0-P0.40-TL-EP3.4", "dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["PAD","e1",0,"",1,"1",0,0,0,null,["RECT",20,30,0],[],0,0,0,1]
["POLY","body",0,"",48,2,[-120,120,"L",120,120,120,-120,-120,-120,-120,120],0]"#}});
        let library = build_pcblib_from_payload(&payload, "RP2350A_C42411118", None).unwrap();
        let component = &library.components[0];
        assert_eq!(library.models.len(), 0);
        assert_eq!(component.bodies.len(), 0);
    }

    #[test]
    fn builds_rotated_rectangular_outline_regions() {
        let outline = rectangular_pad_outline(-57.09, 19.38, 11.811, 39.37, 90.0).unwrap();
        assert_eq!(outline.len(), 5);
        assert_eq!(outline.first(), outline.last());
    }

    #[test]
    fn exports_poly_pad_as_hotspot_with_shape_regions() {
        let payload = json!({"result": {"dataStr": r#"["DOCTYPE","FOOTPRINT","1.8"]
["PAD","e1",0,"",1,"20",-39.37,61.39,0,null,["POLY",[-45.315,76.467,"L",-45.315,51.985,-37.945,44.948,-33.474,44.863,-33.419,76.471,-45.315,76.467]],[],0.003,-0.003,90,1,0,1.9689999999999999,1.9689999999999999,0,0,0]"#}});
        let library =
            build_pcblib_from_payload(&payload, "UFQFPN-20_L3.0-W3.0-P0.50-TL", None).unwrap();
        let component = &library.components[0];
        assert_eq!(component.pads.len(), 1);
        assert_eq!(component.pads[0].shape_top, PAD_SHAPE_ROUND);
        assert_eq!(component.regions.len(), 2);
        assert!(component
            .regions
            .iter()
            .any(|region| region.layer == LAYER_MECHANICAL_9 && region.outline.len() == 6));
        assert_eq!(component.extended_primitive_information.len(), 1);
    }

    #[test]
    fn normalizes_semicolon_footprint_description() {
        assert_eq!(
            normalize_footprint_description(";UFQFPN-20(3x3);UFQFPN-20;UFQFPN-20_L3.0-W3.0-P0.50"),
            Some("UFQFPN-20(3x3); UFQFPN-20; UFQFPN-20_L3.0-W3.0-P0.50".to_string())
        );
    }

    #[test]
    fn parses_rectangular_r_path_command() {
        let shape = json!(["R", -100.001, 35, 200, 70, 0]);
        let points = parse_path_raw_points(&shape);
        assert_eq!(points.len(), 5);
        assert_eq!(
            points,
            vec![
                RawPoint {
                    x: -100.001,
                    y: 35.0
                },
                RawPoint { x: 99.999, y: 35.0 },
                RawPoint {
                    x: 99.999,
                    y: -35.0
                },
                RawPoint {
                    x: -100.001,
                    y: -35.0
                },
                RawPoint {
                    x: -100.001,
                    y: 35.0
                },
            ]
        );
    }
}
