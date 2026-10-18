#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use kurbo::{Affine, Point, Rect, Shape as _};
use vectorcraft_color::{Color, Gradient, GradientKind, GradientPaint, GradientStop, Paint};
use vectorcraft_doc::text::{CharStyle, TextKind, TextObject};
use vectorcraft_doc::{Appearance, AppearanceItem, Artboard, Document, FillLayer, ImageBlob, ImageObject, LayerColor, Node, NodeKind, StrokeLayer};
use vectorcraft_geom::{PathData, shapes};

use super::*;

fn path_node(d: &mut Document, r: Rect, fill: Paint, stroke: Option<(Paint, f64)>) -> Node {
    let mut items = vec![AppearanceItem::Fill(FillLayer::new(fill))];
    if let Some((p, w)) = stroke {
        items.push(AppearanceItem::Stroke(StrokeLayer::new(p, w)));
    }
    Node::path(d.alloc_id(), PathData::from_bezpath(&r.to_path(0.1)), Appearance { items, ..Appearance::default() })
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 40) as u8, (y * 40) as u8, 200, 255]));
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
    out
}

/// Duas pranchetas A4 (595 × 842 pt) lado a lado; arte em duas camadas.
fn sample(with_text: bool) -> Document {
    let mut d = Document::new(595.0, 842.0);
    d.title = "amostra".into();
    d.artboards.push(Artboard {
        id: 2,
        name: "Verso".into(),
        rect: Rect::new(700.0, 0.0, 1295.0, 842.0),
        show_center_mark: false,
        show_cross_hairs: false,
    });
    let red = path_node(
        &mut d,
        Rect::new(100.0, 100.0, 300.0, 200.0),
        Paint::solid(Color::Cmyk { c: 0.0, m: 1.0, y: 1.0, k: 0.0 }),
        Some((Paint::solid(Color::Rgb { r: 0.0, g: 0.0, b: 1.0 }), 2.0)),
    );
    let mut grad = GradientPaint::new(Gradient {
        kind: GradientKind::Linear,
        stops: vec![GradientStop::new(0.0, Color::Rgb { r: 1.0, g: 1.0, b: 0.0 }), GradientStop::new(1.0, Color::Rgb { r: 0.0, g: 0.5, b: 0.0 })],
    });
    grad.angle = 0.0;
    let mut g = path_node(&mut d, Rect::new(120.0, 300.0, 320.0, 400.0), Paint::Gradient(Box::new(grad)), None);
    g.opacity = 0.5;
    // Grupo de recorte: um círculo recorta um retângulo.
    let mut clip = Node::path(d.alloc_id(), PathData::from_bezpath(&kurbo::Circle::new((200.0, 600.0), 50.0).to_path(0.1)), Appearance::default());
    if let NodeKind::Path { clipping, .. } = &mut clip.kind {
        *clipping = true;
    }
    let inner = path_node(&mut d, Rect::new(140.0, 540.0, 260.0, 660.0), Paint::solid(Color::Gray { k: 0.25 }), None);
    let clip_group = Node::new(d.alloc_id(), NodeKind::Group { children: vec![Arc::new(clip), Arc::new(inner)], clip: true });
    let back = path_node(&mut d, Rect::new(800.0, 100.0, 900.0, 200.0), Paint::solid(Color::Rgb { r: 0.2, g: 0.4, b: 0.6 }), None);
    let group = Node::group(d.alloc_id(), vec![Arc::new(red), Arc::new(g)]);
    let layer = d.layers[0].id;
    Arc::make_mut(&mut d.layers[0]).name = Some("Fundo".into());
    let mut top = Node::layer(d.alloc_id(), "Frente", LayerColor::Preset(1));
    let bytes = png(4, 3);
    let blob = ImageBlob::new("image/png", bytes);
    let key = blob.content_key();
    d.images.insert(key.clone(), blob);
    let im = ImageObject {
        key,
        width: 4,
        height: 3,
        xf: Affine::translate((400.0, 100.0)) * Affine::scale(10.0),
        link: None,
        placement: Default::default(),
    };
    let mut kids = vec![Arc::new(clip_group), Arc::new(Node::new(d.alloc_id(), NodeKind::Image(Box::new(im))))];
    if with_text {
        let style =
            CharStyle { font_family: "Arial".into(), size: 24.0, fill: Paint::solid(Color::Rgb { r: 0.0, g: 0.0, b: 0.0 }), ..CharStyle::default() };
        let t = TextObject::point(Point::new(100.0, 750.0), "Olá CDR", style.clone());
        kids.push(Arc::new(Node::new(d.alloc_id(), NodeKind::Text(Box::new(t)))));
        let mut area = TextObject::point(Point::new(350.0, 700.0), "Texto de parágrafo", style);
        area.kind = TextKind::Area { frame: shapes::rectangle(Rect::new(0.0, 0.0, 200.0, 80.0)) };
        kids.push(Arc::new(Node::new(d.alloc_id(), NodeKind::Text(Box::new(area)))));
    }
    if let Some(c) = top.children_mut() {
        *c = kids;
    }
    let l0 = Arc::make_mut(&mut d.layers[0]);
    assert_eq!(l0.id, layer);
    if let Some(c) = l0.children_mut() {
        c.push(Arc::new(group));
        c.push(Arc::new(back));
    }
    d.layers.push(Arc::new(top));
    d
}

fn paths(d: &Document) -> Vec<Rect> {
    let mut v = Vec::new();
    d.walk(|n| {
        if let NodeKind::Path { path, clipping: false, .. } = &n.kind {
            v.push(path.to_bezpath().bounding_box());
        }
    });
    v
}

fn close(a: Rect, b: Rect) -> bool {
    (a.x0 - b.x0).abs() < 0.05 && (a.y0 - b.y0).abs() < 0.05 && (a.x1 - b.x1).abs() < 0.05 && (a.y1 - b.y1).abs() < 0.05
}

#[test]
fn every_version_round_trips_and_validates() {
    let doc = sample(false);
    for v in OLDEST_EXPORT_VERSION..=NEWEST_EXPORT_VERSION {
        let out = export(&doc, &ExportOptions { version: v, ..ExportOptions::default() }).unwrap_or_else(|e| panic!("v{v}: {e}"));
        assert!(out.validation.as_deref().unwrap().starts_with("validado"), "v{v}");
        let info = detect_version(&out.bytes).unwrap();
        assert_eq!(info.version, v);
        let expected = match v {
            ..=13 => Container::Riff,
            14 | 15 => Container::ZipRiffData,
            _ => Container::ZipRootDat,
        };
        assert_eq!(info.container, expected, "v{v}");
        assert!(validate(&out.bytes).ok(), "v{v}");
        let back = import(&out.bytes, "volta", &ImportOptions::default()).unwrap();
        let d = &back.document;
        assert_eq!(d.artboards.len(), 2, "v{v}");
        assert!(close(d.artboards[0].rect, Rect::new(0.0, 0.0, 595.0, 842.0)), "v{v}: {:?}", d.artboards[0].rect);
        assert!((d.artboards[1].rect.width() - 595.0).abs() < 0.05, "v{v}");
        let p = paths(d);
        for want in [Rect::new(100.0, 100.0, 300.0, 200.0), Rect::new(120.0, 300.0, 320.0, 400.0), Rect::new(150.0, 550.0, 250.0, 650.0)] {
            assert!(p.iter().any(|r| close(*r, want)), "v{v}: {want:?} in {p:?}");
        }
        // O objeto da segunda prancheta foi para a segunda página e volta no mesmo lugar dela.
        let second = d.artboards[1].rect;
        assert!(p.iter().any(|r| close(*r, Rect::new(second.x0 + 100.0, 100.0, second.x0 + 200.0, 200.0))), "v{v}: {p:?}");
        let mut names = Vec::new();
        d.walk(|n| {
            if n.is_layer() {
                names.push(n.name.clone().unwrap_or_default());
            }
        });
        assert!(names.iter().any(|n| n.ends_with("Fundo")) && names.iter().any(|n| n.ends_with("Frente")), "v{v}: {names:?}");
    }
}

#[test]
fn colours_strokes_clips_images_and_opacity_survive() {
    let out = export(&sample(false), &ExportOptions::default()).unwrap();
    let d = import(&out.bytes, "volta", &ImportOptions::default()).unwrap().document;
    let mut cmyk = false;
    let mut blue_stroke = false;
    let mut gradient_half = false;
    let mut clip_group = false;
    let _ = Rect::ZERO;
    let mut image = None;
    d.walk(|n| {
        for i in &n.appearance.items {
            match i {
                AppearanceItem::Fill(f) => {
                    if let Paint::Solid { color: Color::Cmyk { m, y, .. }, .. } = f.paint {
                        cmyk |= m > 0.99 && y > 0.99;
                    }
                    if matches!(f.paint, Paint::Gradient(_)) {
                        gradient_half |= (n.opacity - 0.5).abs() < 0.02;
                    }
                }
                AppearanceItem::Stroke(s) => {
                    if let Paint::Solid { color: Color::Rgb { b, .. }, .. } = s.paint {
                        blue_stroke |= b > 0.99 && (s.width - 2.0).abs() < 0.05;
                    }
                }
            }
        }
        // O recorte foi aplicado à geometria: o retângulo cinza ficou dentro do círculo.
        if let NodeKind::Path { path, .. } = &n.kind
            && n.appearance
                .items
                .iter()
                .any(|i| matches!(i, AppearanceItem::Fill(f) if matches!(f.paint, Paint::Solid { color: Color::Gray { .. }, .. })))
        {
            let b = path.to_bezpath().bounding_box();
            clip_group |= b.x0 >= 149.0 && b.x1 <= 251.0 && b.y0 >= 549.0 && b.y1 <= 651.0 && b.width() > 99.0;
        }
        if let NodeKind::Image(im) = &n.kind {
            image = Some((im.width, im.height, im.xf));
        }
    });
    assert!(cmyk, "CMYK fill");
    assert!(blue_stroke, "stroke colour and width");
    assert!(gradient_half, "gradient with 50% transparency");
    assert!(clip_group, "clip applied to the geometry");
    let (w, h, xf) = image.unwrap();
    assert_eq!((w, h), (4, 3));
    let r = xf.transform_rect_bbox(Rect::new(0.0, 0.0, 4.0, 3.0));
    assert!(close(r, Rect::new(400.0, 100.0, 440.0, 130.0)), "{r:?}");
}

#[test]
fn text_stays_editable_from_version_16() {
    let doc = sample(true);
    for v in [16, 19, 22] {
        let out = export(&doc, &ExportOptions { version: v, ..ExportOptions::default() }).unwrap();
        let d = import(&out.bytes, "t", &ImportOptions::default()).unwrap();
        let mut texts = Vec::new();
        d.document.walk(|n| {
            if let NodeKind::Text(t) = &n.kind {
                texts.push((
                    t.runs.iter().map(|r| r.text.as_str()).collect::<String>(),
                    t.runs.first().map(|r| r.style.size),
                    matches!(t.kind, TextKind::Area { .. }),
                ));
            }
        });
        assert!(texts.iter().any(|(s, size, _)| s.contains("Olá CDR") && size.is_some_and(|z| (z - 24.0).abs() < 0.1)), "v{v}: {texts:?}");
        assert!(!texts.iter().any(|(s, _, _)| s.contains("parágrafo")), "area type goes as outlines: {texts:?}");
        assert!(out.warnings.iter().any(|w| w.contains("texto de área")));
        assert!(d.fonts.iter().any(|f| f == "Arial"));
    }
}

#[test]
fn experimental_versions_say_so() {
    let out = export(&sample(true), &ExportOptions { version: 12, ..ExportOptions::default() }).unwrap();
    assert!(out.warnings.iter().any(|w| w.contains("experimental")));
    assert!(out.validation.unwrap().contains("experimental"));
}

#[test]
fn bad_versions_and_documents_are_refused() {
    let d = sample(false);
    assert!(matches!(export(&d, &ExportOptions { version: 6, ..ExportOptions::default() }), Err(Error::Export(_))));
    assert!(matches!(export(&d, &ExportOptions { version: 23, ..ExportOptions::default() }), Err(Error::Export(_))));
    let mut none = d.clone();
    none.artboards.clear();
    assert!(matches!(export(&none, &ExportOptions::default()), Err(Error::Export(_))));
}

#[test]
fn import_options_merge_layers_scale_and_outline() {
    let out = export(&sample(false), &ExportOptions::default()).unwrap();
    let merged = import(&out.bytes, "m", &ImportOptions { preserve_layers: false, ..ImportOptions::default() }).unwrap();
    assert_eq!(merged.document.layers.len(), 2, "uma camada por página");
    let half = import(&out.bytes, "h", &ImportOptions { scale: 0.5, ..ImportOptions::default() }).unwrap();
    assert!((half.document.artboards[0].rect.width() - 297.5).abs() < 0.05);
}

#[test]
fn not_cdr_and_damaged_files_never_panic() {
    assert_eq!(detect_version(b"hello"), Err(Error::NotCdr));
    assert_eq!(detect_version(b""), Err(Error::NotCdr));
    assert!(!is_cdr(b"PK\x03\x04word/document.xml"));
    assert!(!validate(b"RIFF\x10\x00\x00\x00WAVEfmt ").ok());
    let good = export(&sample(true), &ExportOptions { version: 12, ..ExportOptions::default() }).unwrap().bytes;
    let zip = export(&sample(true), &ExportOptions { version: 20, ..ExportOptions::default() }).unwrap().bytes;
    for bytes in [&good, &zip] {
        // Truncado em vários pontos e com bytes trocados: erro ou documento, nunca pânico.
        for cut in [12, 40, 100, bytes.len() / 3, bytes.len() / 2, bytes.len() - 1] {
            let b = &bytes[..cut.min(bytes.len())];
            let _ = validate(b);
            let _ = import(b, "x", &ImportOptions::default());
        }
        let mut seed = 0x1234_5678u32;
        for _ in 0..40 {
            let mut b = bytes.clone();
            for _ in 0..16 {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                let i = 12 + (seed as usize) % (b.len() - 12);
                b[i] ^= (seed >> 16) as u8 | 1;
            }
            let _ = validate(&b);
            let _ = import(&b, "x", &ImportOptions::default());
        }
    }
    // Cabeçalho CDR com corpo lixo.
    let mut junk = b"RIFF\xff\xff\x00\x00CDRJ".to_vec();
    junk.extend(std::iter::repeat_n(0xAB, 300));
    let _ = import(&junk, "x", &ImportOptions::default());
}

#[test]
fn matrix_covers_the_fidelity_list_without_overclaiming() {
    assert!(matrix::FEATURES.len() >= 13, "{}", FEATURES_LEN);
    let md = matrix::markdown();
    assert!(md.contains("Gradientes") && md.contains("Convertida com limitações"));
    // Nada com perdas conhecidas aparece como total.
    for f in matrix::FEATURES {
        if f.feature.contains("Gradientes") || f.feature.contains("Efeitos") || f.feature.contains("texto") {
            assert_ne!(f.export, matrix::Level::Full, "{}", f.feature);
        }
    }
    let j = matrix::to_json();
    assert_eq!(j["exportVersions"].as_array().unwrap().len(), usize::from(NEWEST_EXPORT_VERSION - OLDEST_EXPORT_VERSION + 1));
}

const FEATURES_LEN: usize = matrix::FEATURES.len();

/// `docs/cdr-compatibility.md` traz a matriz gerada daqui. `VC_WRITE_CDR_DOC=1` a reescreve.
#[test]
fn compatibility_doc_matches_the_matrix() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/cdr-compatibility.md");
    let doc = std::fs::read_to_string(path).unwrap_or_default();
    let start = "<!-- matriz:inicio -->\n";
    let end = "<!-- matriz:fim -->";
    let table = matrix::markdown();
    if std::env::var_os("VC_WRITE_CDR_DOC").is_some() {
        let (a, b) = (doc.find(start).unwrap() + start.len(), doc.find(end).unwrap());
        std::fs::write(path, format!("{}{}{}", &doc[..a], table, &doc[b..])).unwrap();
        return;
    }
    assert!(doc.contains(&table), "docs/cdr-compatibility.md está desatualizado: rode com VC_WRITE_CDR_DOC=1");
}
