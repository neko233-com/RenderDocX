//! Presentation of the FrameStatistics data exposed by the matching replay bridge.
use crate::i18n::Language;
use crate::replay::Snapshot;
use serde_json::Value;

pub const OVERVIEW: usize = 14;

pub const CATEGORIES: [&str; 14] = [
    "draws",
    "dispatches",
    "indices",
    "layouts",
    "vertices",
    "updates",
    "constants",
    "samplers",
    "resources",
    "shaders",
    "blends",
    "depths",
    "rasters",
    "outputs",
];

pub fn category_label(index: usize, language: Language) -> &'static str {
    match index {
        OVERVIEW => language.text("Overview", "概览"),
        0 => language.text("Draws", "绘制"),
        1 => language.text("Dispatches", "分发"),
        2 => language.text(
            "Index bindings",
            "索引绑定",
        ),
        3 => language.text(
            "Layout bindings",
            "布局绑定",
        ),
        4 => language.text("Vertex bindings", "顶点绑定"),
        5 => language.text(
            "Resource updates",
            "资源更新",
        ),
        6 => language.text(
            "Constant bindings",
            "常量绑定",
        ),
        7 => language.text(
            "Sampler bindings",
            "采样器绑定",
        ),
        8 => language.text(
            "Resource bindings",
            "资源绑定",
        ),
        9 => language.text(
            "Shader bindings",
            "着色器绑定",
        ),
        10 => language.text("Blend state", "混合状态"),
        11 => language.text(
            "Depth/stencil state",
            "深度/模板状态",
        ),
        12 => language.text("Rasterization", "光栅化"),
        13 => language.text("Output bindings", "输出绑定"),
        _ => language.text("Unavailable", "不可用"),
    }
}

/// Summary follows StatisticsViewer's action flags and resource classification.
/// Incomplete or invalid bridge data remains unavailable instead of becoming zero.
pub fn overview(snapshot: &Snapshot, language: Language) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut metric = |name: &'static str, value: Option<String>| {
        rows.push(Row::Metric {
            name: name.into(),
            value: value.unwrap_or_else(|| "—".into()),
        })
    };
    let recorded = snapshot
        .frame_statistics
        .as_ref()
        .and_then(|stats| stats.get("recorded"))
        .and_then(Value::as_bool)
        .map(|recorded| {
            if recorded {
                language.text("Available", "可用")
            } else {
                language.text("Not recorded", "未记录")
            }
            .to_owned()
        });
    metric(
        language.text("Detailed statistics", "详细统计"),
        recorded,
    );
    if let Some(info) = &snapshot.frame_info {
        for (name, value) in [
            (
                language.text(
                    "Compressed capture bytes",
                    "捕获压缩字节数",
                ),
                &info.compressed_size,
            ),
            (
                language.text(
                    "Uncompressed capture bytes",
                    "捕获未压缩字节数",
                ),
                &info.uncompressed_size,
            ),
            (
                language.text(
                    "Persistent bytes (approx.)",
                    "持久数据字节数（近似）",
                ),
                &info.persistent_size,
            ),
            (
                language.text(
                    "Initial data bytes (approx.)",
                    "初始数据字节数（近似）",
                ),
                &info.initial_data_size,
            ),
        ] {
            metric(name, value.parse::<u64>().ok().map(|v| v.to_string()));
        }
    }
    let draws = snapshot
        .actions
        .iter()
        .filter(|a| a.flags & 0x0a != 0)
        .count();
    let dispatches = snapshot
        .actions
        .iter()
        .filter(|a| a.flags & 0x04 != 0)
        .count();
    let diagnostics = snapshot
        .actions
        .iter()
        .filter(|a| a.flags & 0xe0 != 0)
        .count();
    let api_calls = snapshot.actions.last().and_then(|a| {
        (a.event_id as usize).checked_sub(draws.checked_add(dispatches)?.checked_add(diagnostics)?)
    });
    metric(
        language.text("Draw calls", "绘制调用"),
        Some(draws.to_string()),
    );
    metric(
        language.text(
            "Dispatch calls",
            "分发调用",
        ),
        Some(dispatches.to_string()),
    );
    metric(
        language.text("API calls", "API 调用"),
        api_calls.map(|v| v.to_string()),
    );
    metric(
        language.text(
            "API calls per draw/dispatch",
            "每次绘制/分发的 API 调用数",
        ),
        api_calls.and_then(|v| {
            let n = draws.checked_add(dispatches)?;
            (n > 0).then(|| format!("{:.2}", v as f64 / n as f64))
        }),
    );
    if let Some(stats) = snapshot
        .frame_statistics
        .as_ref()
        .filter(|s| s.get("recorded").and_then(Value::as_bool) == Some(true))
    {
        let calls = |keys: &[&str]| -> Option<String> {
            keys.iter()
                .try_fold(0u64, |sum, key| {
                    let section = stats.get(*key)?;
                    if let Some(stages) = section.as_array() {
                        if stages.len() != 14 {
                            return None;
                        }
                        stages.iter().try_fold(sum, |sum, stage| {
                            sum.checked_add(stage.get("calls")?.as_u64()?)
                        })
                    } else {
                        sum.checked_add(section.get("calls")?.as_u64()?)
                    }
                })
                .map(|sum| sum.to_string())
        };
        for (name, keys) in [
            (
                language.text(
                    "Index/vertex bind calls",
                    "索引/顶点绑定调用",
                ),
                &["indices", "vertices", "layouts"][..],
            ),
            (
                language.text(
                    "Constant bind calls",
                    "常量绑定调用",
                ),
                &["constants"][..],
            ),
            (
                language.text(
                    "Sampler bind calls",
                    "采样器绑定调用",
                ),
                &["samplers"][..],
            ),
            (
                language.text(
                    "Resource bind calls",
                    "资源绑定调用",
                ),
                &["resources"][..],
            ),
            (
                language.text(
                    "Shader set calls",
                    "着色器设置调用",
                ),
                &["shaders"][..],
            ),
            (
                language.text(
                    "Blend set calls",
                    "混合状态设置调用",
                ),
                &["blends"][..],
            ),
            (
                language.text(
                    "Depth/stencil set calls",
                    "深度/模板状态设置调用",
                ),
                &["depths"][..],
            ),
            (
                language.text(
                    "Rasterization set calls",
                    "光栅化状态设置调用",
                ),
                &["rasters"][..],
            ),
            (
                language.text(
                    "Resource update calls",
                    "资源更新调用",
                ),
                &["updates"][..],
            ),
            (
                language.text(
                    "Output set calls",
                    "输出设置调用",
                ),
                &["outputs"][..],
            ),
        ] {
            metric(name, calls(keys));
        }
    }
    metric(
        language.text("Textures", "纹理数"),
        Some(snapshot.texture_count.to_string()),
    );
    metric(
        language.text("Buffers", "缓冲区数"),
        Some(snapshot.buffer_count.to_string()),
    );
    let texture_complete = snapshot.textures.len() == snapshot.texture_count;
    let buffer_complete = snapshot.buffers.len() == snapshot.buffer_count;
    let mut tex = 0u128;
    let mut rt = 0u128;
    let mut large = 0u128;
    let mut tex_n = 0u128;
    let mut rt_n = 0u128;
    let mut large_n = 0u128;
    let mut width = 0u128;
    let mut height = 0u128;
    let mut large_width = 0u128;
    let mut large_height = 0u128;
    let valid_textures = texture_complete
        && snapshot.textures.iter().all(|t| {
            let Ok(bytes) = t.bytes.parse::<u64>() else {
                return false;
            };
            if t.creation_flags & 0x6 != 0 {
                rt += bytes as u128;
                rt_n += 1;
            } else {
                tex += bytes as u128;
                tex_n += 1;
                width += t.width as u128;
                height += t.height as u128;
                if t.width > 32 && t.height > 32 {
                    large += bytes as u128;
                    large_n += 1;
                    large_width += t.width as u128;
                    large_height += t.height as u128;
                }
            }
            true
        });
    let average = |w: u128, h: u128, n: u128| {
        if n == 0 {
            "0 × 0".into()
        } else {
            format!("{:.2} × {:.2}", w as f64 / n as f64, h as f64 / n as f64)
        }
    };
    metric(
        language.text(
            "Render targets",
            "渲染目标数",
        ),
        valid_textures.then(|| rt_n.to_string()),
    );
    metric(
        language.text(
            "Texture bytes (excluding targets)",
            "纹理字节数（不含渲染目标）",
        ),
        valid_textures.then(|| tex.to_string()),
    );
    metric(
        language.text(
            "Texture bytes over 32×32",
            "大于 32×32 的纹理字节数",
        ),
        valid_textures.then(|| large.to_string()),
    );
    metric(
        language.text(
            "Render target bytes",
            "渲染目标字节数",
        ),
        valid_textures.then(|| rt.to_string()),
    );
    metric(
        language.text(
            "Average texture dimensions",
            "纹理平均尺寸",
        ),
        valid_textures.then(|| average(width, height, tex_n)),
    );
    metric(
        language.text(
            "Average dimensions over 32×32",
            "大于 32×32 的纹理平均尺寸",
        ),
        valid_textures.then(|| average(large_width, large_height, large_n)),
    );
    let mut buf = 0u128;
    let mut vb = 0u128;
    let mut ib = 0u128;
    let valid_buffers = buffer_complete
        && snapshot.buffers.iter().all(|b| {
            let Ok(bytes) = b.length.parse::<u64>() else {
                return false;
            };
            buf += bytes as u128;
            if b.creation_flags & 1 != 0 {
                vb += bytes as u128;
            }
            if b.creation_flags & 2 != 0 {
                ib += bytes as u128;
            }
            true
        });
    metric(
        language.text(
            "Buffer bytes",
            "缓冲区字节数",
        ),
        valid_buffers.then(|| buf.to_string()),
    );
    metric(
        language.text(
            "Vertex buffer bytes",
            "顶点缓冲区字节数",
        ),
        valid_buffers.then(|| vb.to_string()),
    );
    metric(
        language.text(
            "Index buffer bytes",
            "索引缓冲区字节数",
        ),
        valid_buffers.then(|| ib.to_string()),
    );
    metric(
        language.text(
            "Total GPU resource bytes",
            "GPU 资源总字节数",
        ),
        (valid_textures && valid_buffers).then(|| (tex + rt + buf).to_string()),
    );
    rows
}

/// Human-readable UTF-8 export shares the same localized rows as the GUI.
pub fn text_report(snapshot: &Snapshot, language: Language) -> String {
    use std::fmt::Write;
    let mut text = format!(
        "{}: {}\n",
        language.text("Frame", "帧"),
        snapshot.frame
    );
    let mut append = |title: &str, rows: Vec<Row>| {
        let _ = writeln!(text, "\n{title}\n");
        for row in rows {
            match row {
                Row::Heading(name) => {
                    let _ = writeln!(text, "\n{name}");
                }
                Row::Metric { name, value } => {
                    let _ = writeln!(text, "{name}: {value}");
                }
                Row::Bucket { label, count, .. } => {
                    let _ = writeln!(text, "{label}: {count}");
                }
            }
        }
    };
    append(
        category_label(OVERVIEW, language),
        overview(snapshot, language),
    );
    if let Some(stats) = snapshot
        .frame_statistics
        .as_ref()
        .filter(|s| s.get("recorded").and_then(Value::as_bool) == Some(true))
    {
        for index in 0..CATEGORIES.len() {
            append(
                category_label(index, language),
                report(stats, index, language).unwrap_or_else(|| {
                    vec![Row::Metric {
                        name: language
                            .text(
                                "Unavailable or invalid",
                                "不可用或无效",
                            )
                            .into(),
                        value: "—".into(),
                    }]
                }),
            );
        }
    }
    text
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Heading(String),
    Metric {
        name: String,
        value: String,
    },
    Bucket {
        label: String,
        count: u64,
        maximum: u64,
    },
}

fn stage_label(index: usize, language: Language) -> String {
    let label = match index {
        0 => language.text("Vertex", "顶点"),
        1 => language.text(
            "Hull / Tessellation control",
            "外壳 / 曲面细分控制",
        ),
        2 => language.text(
            "Domain / Tessellation evaluation",
            "域 / 曲面细分求值",
        ),
        3 => language.text("Geometry", "几何"),
        4 => language.text(
            "Pixel / Fragment",
            "像素 / 片元",
        ),
        5 => language.text("Compute", "计算"),
        6 => "Task / Amplification",
        7 => "Mesh",
        8 => "RayGen",
        9 => "Intersection",
        10 => "AnyHit",
        11 => "ClosestHit",
        12 => "Miss",
        13 => "Callable",
        _ => return format!("#{index}"),
    };
    label.to_owned()
}

fn metric_label(key: &str, language: Language) -> &'static str {
    match key {
        "calls" => language.text("Calls", "调用次数"),
        "sets" => language.text(
            "Non-null sets",
            "非空绑定数",
        ),
        "nulls" => language.text("Null sets", "空绑定数"),
        "redundants" => language.text(
            "Redundant sets",
            "冗余绑定数",
        ),
        "instanced" => language.text("Instanced", "实例化"),
        "indirect" => language.text("Indirect", "间接调用"),
        "clients" => language.text("CPU updates", "CPU 更新"),
        "servers" => language.text(
            "Command queue updates",
            "命令队列更新",
        ),
        _ => "",
    }
}

fn histogram_label(key: &str, language: Language) -> &'static str {
    match key {
        "counts" => language.text(
            "Instances per draw",
            "每次绘制的实例数",
        ),
        "bindslots" => language.text(
            "Bindings per call",
            "每次调用的绑定数",
        ),
        "sizes" => language.text("Size buckets", "大小分桶"),
        "types" => language.text(
            "Resource types",
            "资源类型",
        ),
        "viewports" => language.text(
            "Viewports per call",
            "每次调用的视口数",
        ),
        "rects" => language.text(
            "Scissors per call",
            "每次调用的裁剪矩形数",
        ),
        _ => "",
    }
}

fn bucket_label(key: &str, index: usize, count: usize) -> String {
    const TYPES: [&str; 12] = [
        "Unknown",
        "Buffer",
        "Texture1D",
        "Texture1DArray",
        "Texture2D",
        "TextureRect",
        "Texture2DArray",
        "Texture2DMS",
        "Texture2DMSArray",
        "Texture3D",
        "TextureCube",
        "TextureCubeArray",
    ];
    match key {
        "types" => TYPES
            .get(index)
            .map_or_else(|| format!("#{index}"), |name| (*name).to_owned()),
        "sizes" => format!("2^{index} B"),
        "counts" if index + 1 == count => format!(">={index}"),
        _ => index.to_string(),
    }
}

fn append_object(rows: &mut Vec<Row>, object: &Value, language: Language) {
    for key in [
        "calls",
        "sets",
        "nulls",
        "redundants",
        "instanced",
        "indirect",
        "clients",
        "servers",
    ] {
        if let Some(value) = object.get(key) {
            rows.push(Row::Metric {
                name: metric_label(key, language).into(),
                value: value
                    .as_u64()
                    .map_or_else(|| "—".into(), |value| value.to_string()),
            });
        }
    }
    for key in [
        "counts",
        "bindslots",
        "sizes",
        "types",
        "viewports",
        "rects",
    ] {
        let Some(raw) = object.get(key) else {
            continue;
        };
        rows.push(Row::Heading(histogram_label(key, language).into()));
        let values: Option<Vec<u64>> = raw
            .as_array()
            .and_then(|values| values.iter().map(Value::as_u64).collect());
        let Some(values) = values else {
            rows.push(Row::Metric {
                name: language
                    .text("Invalid data", "数据无效")
                    .into(),
                value: "—".into(),
            });
            continue;
        };
        if values.is_empty() {
            rows.push(Row::Metric {
                name: language
                    .text("No buckets", "没有分桶")
                    .into(),
                value: "—".into(),
            });
            continue;
        }
        let maximum = values.iter().copied().max().unwrap_or(0);
        if maximum == 0 {
            rows.push(Row::Metric {
                name: language
                    .text(
                        "No recorded counts",
                        "没有记录的计数",
                    )
                    .into(),
                value: "0".into(),
            });
            continue;
        }
        for (index, &count) in values.iter().enumerate().filter(|(_, count)| **count != 0) {
            rows.push(Row::Bucket {
                label: bucket_label(key, index, values.len()),
                count,
                maximum,
            });
        }
    }
}

fn aggregate_stages(stages: &[Value]) -> Option<Value> {
    // The matching RenderDoc ShaderStage enum has fourteen entries. Do not
    // present an incomplete or mismatched set as an all-stage total.
    if stages.len() != 14 {
        return None;
    }
    let first = stages.first()?.as_object()?;
    let mut total = serde_json::Map::new();
    for (key, initial) in first {
        if initial.is_number() {
            let sum = stages.iter().try_fold(0u64, |sum, stage| {
                sum.checked_add(stage.get(key)?.as_u64()?)
            })?;
            total.insert(key.clone(), Value::from(sum));
        } else if let Some(array) = initial.as_array() {
            let mut sums = vec![0u64; array.len()];
            for stage in stages {
                let values = stage.get(key)?.as_array()?;
                if values.len() != sums.len() {
                    return None;
                }
                for (sum, value) in sums.iter_mut().zip(values) {
                    *sum = sum.checked_add(value.as_u64()?)?;
                }
            }
            total.insert(key.clone(), Value::from(sums));
        } else {
            return None;
        }
    }
    Some(Value::Object(total))
}

/// Missing and malformed sections return None; the caller must not substitute zero.
pub fn report(stats: &Value, category: usize, language: Language) -> Option<Vec<Row>> {
    if stats.get("recorded")?.as_bool()? != true {
        return None;
    }
    let value = stats.get(*CATEGORIES.get(category)?)?;
    let mut rows = Vec::new();
    if let Some(stages) = value.as_array() {
        if stages.is_empty() {
            return None;
        }
        for (index, stage) in stages.iter().enumerate() {
            if !stage.is_object() {
                return None;
            }
            rows.push(Row::Heading(stage_label(index, language)));
            append_object(&mut rows, stage, language);
        }
        if let Some(total) = aggregate_stages(stages) {
            rows.push(Row::Heading(
                language
                    .text(
                        "All stages",
                        "所有阶段合计",
                    )
                    .into(),
            ));
            append_object(&mut rows, &total, language);
        }
    } else if value.is_object() {
        append_object(&mut rows, value, language);
    } else {
        return None;
    }
    (!rows.is_empty()).then_some(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recorded_data_preserves_counts_and_bucket_semantics() {
        let stats = serde_json::json!({"recorded":true,"draws":{"calls":4,"instanced":3,"counts":[0,2,0,1]},"constants":[{"calls":2,"sizes":[0,3],"bindslots":[1,2]}],"updates":{"sizes":[0,"bad"]}});
        let draw = report(&stats, 0, Language::English).unwrap();
        assert!(draw.contains(&Row::Metric {
            name: "Calls".into(),
            value: "4".into()
        }));
        assert!(draw.contains(&Row::Bucket {
            label: ">=3".into(),
            count: 1,
            maximum: 2
        }));
        let constants = report(&stats, 6, Language::Chinese).unwrap();
        assert!(constants.contains(&Row::Bucket {
            label: "2^1 B".into(),
            count: 3,
            maximum: 3
        }));
        assert!(
            report(&stats, 5, Language::English)
                .unwrap()
                .iter()
                .any(|row| matches!(row, Row::Metric { name, .. } if name == "Invalid data"))
        );
        assert!(
            report(
                &serde_json::json!({"recorded":false,"draws":{"calls":0}}),
                0,
                Language::English
            )
            .is_none()
        );
        assert!(report(&serde_json::json!({"recorded":true}), 0, Language::English).is_none());
    }

    #[test]
    fn empty_and_invalid_histograms_are_not_zeros() {
        let stats = serde_json::json!({"recorded":true,"draws":{"counts":[]},"updates":{"sizes":123},"constants":[]});
        assert!(
            report(&stats, 0, Language::English)
                .unwrap()
                .contains(&Row::Metric {
                    name: "No buckets".into(),
                    value: "—".into()
                })
        );
        assert!(
            report(&stats, 5, Language::English)
                .unwrap()
                .contains(&Row::Metric {
                    name: "Invalid data".into(),
                    value: "—".into()
                })
        );
        assert!(report(&stats, 6, Language::English).is_none());
    }

    #[test]
    fn overview_uses_upstream_flags_and_refuses_partial_resource_totals() {
        let mut snapshot: Snapshot = serde_json::from_value(serde_json::json!({
            "protocol": 1, "api": 0, "frame": 1, "degraded": false,
            "frame_info": {"file_offset":"0", "uncompressed_size":"100", "compressed_size":"50", "persistent_size":"8", "initial_data_size":"4", "capture_time":"0", "contains_annotations":false},
            "actions": [
                {"event_id":2,"action_id":1,"depth":0,"parent":-1,"flags":2,"indices":0,"instances":1,"name":"draw"},
                {"event_id":4,"action_id":2,"depth":0,"parent":-1,"flags":4,"indices":0,"instances":0,"name":"dispatch"},
                {"event_id":7,"action_id":3,"depth":0,"parent":-1,"flags":32,"indices":0,"instances":0,"name":"marker"}
            ],
            "resource_count":3,"texture_count":2,"buffer_count":1,
            "textures":[
                {"name":"tex","width":64,"height":64,"depth":1,"dimension":2,"mips":1,"array_size":1,"samples":1,"bytes":"100","creation_flags":1,"format_type":0,"component_type":0,"component_count":4,"component_byte_width":1},
                {"name":"rt","width":128,"height":128,"depth":1,"dimension":2,"mips":1,"array_size":1,"samples":1,"bytes":"200","creation_flags":2,"format_type":0,"component_type":0,"component_count":4,"component_byte_width":1}
            ],
            "buffers":[{"name":"both","resource_index":null,"length":"30","creation_flags":3}]
        })).unwrap();
        let rows = overview(&snapshot, Language::English);
        for (name, value) in [
            ("Draw calls", "1"),
            ("Dispatch calls", "1"),
            ("API calls", "4"),
            ("Render targets", "1"),
            ("Texture bytes (excluding targets)", "100"),
            ("Render target bytes", "200"),
            ("Vertex buffer bytes", "30"),
            ("Index buffer bytes", "30"),
            ("Total GPU resource bytes", "330"),
        ] {
            assert!(
                rows.contains(&Row::Metric {
                    name: name.into(),
                    value: value.into()
                }),
                "{name}"
            );
        }
        let report = text_report(&snapshot, Language::English);
        assert!(report.contains("Total GPU resource bytes: 330"));
        snapshot.frame_statistics = Some(
            serde_json::json!({"recorded": true, "constants": vec![serde_json::json!({"calls": 2}); 14]}),
        );
        let rows = overview(&snapshot, Language::English);
        assert!(rows.contains(&Row::Metric {
            name: "Constant bind calls".into(),
            value: "28".into()
        }));
        assert!(rows.contains(&Row::Metric {
            name: "Sampler bind calls".into(),
            value: "—".into()
        }));
        let mut partial = snapshot;
        partial.texture_count += 1;
        assert!(
            overview(&partial, Language::English).contains(&Row::Metric {
                name: "Total GPU resource bytes".into(),
                value: "—".into()
            })
        );
    }

    #[test]
    fn stage_totals_require_every_stage_and_matching_buckets() {
        let stages = vec![serde_json::json!({"calls":2,"bindslots":[1,3]}); 14];
        let total = aggregate_stages(&stages).unwrap();
        assert_eq!(total["calls"], 28);
        assert_eq!(total["bindslots"], serde_json::json!([14, 42]));
        assert!(aggregate_stages(&stages[..13]).is_none());
        let mut invalid = stages;
        invalid[13]["bindslots"] = serde_json::json!([1]);
        assert!(aggregate_stages(&invalid).is_none());
    }
}
