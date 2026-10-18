//! Leitura: o documento que `tracedraw-cdr` monta (milímetros, origem no canto inferior esquerdo da
//! página, Y para cima) convertido em arte editável do VectorCraft (pontos, Y para baixo), uma
//! prancheta por página, lado a lado.

use std::collections::BTreeSet;
use std::sync::Arc;

use kurbo::{Affine, PathEl, Point, Rect, Shape as _, Vec2};
use tracedraw_core::document::{Layer as TdLayer, Shape as TdShape, ShapeKind, TextSpan};
use tracedraw_core::style::{Arrowhead, FountainKind, LineCap as TdCap, LineJoin as TdJoin};
use tracedraw_core::{Color as TdColor, Document as TdDocument, Fill, Stroke, TextAlign};
use vectorcraft_color::{Color, Gradient, GradientGeom, GradientKind, GradientPaint, GradientStop, Paint};
use vectorcraft_doc::text::{CharStyle, Justify, TextKind, TextObject, TextRun};
use vectorcraft_doc::{
    Appearance, AppearanceItem, Artboard, Dash, Document, FillLayer, ImageBlob, ImageObject, LayerColor, LineCap, LineJoin, Node, NodeId, NodeKind,
    StrokeLayer, Unit,
};
use vectorcraft_geom::{FillRule, PathData, shapes};

use crate::{CdrInfo, Error, Result, guarded};

/// Pontos por milímetro.
pub(crate) const PT_PER_MM: f64 = 72.0 / 25.4;
/// Distância entre pranchetas de páginas vizinhas (pt).
const PAGE_GAP: f64 = 36.0;
/// Mais objetos que uma importação cria (um arquivo hostil não esgota a memória com eles).
const MAX_OBJECTS: usize = 1 << 20;
/// Profundidade máxima de grupos aninhados lida.
const MAX_DEPTH: usize = 64;
/// Maior imagem embutida aceita (pixels por lado).
const MAX_IMAGE_SIDE: u32 = 30_000;
/// Espessura de um contorno "fio de cabelo" (0,0762 mm).
const HAIRLINE_PT: f64 = 0.216;

/// Preferências de importação.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportOptions {
    /// Escala da arte: 1 = tamanho físico original (mm do CorelDRAW em pt).
    pub scale: f64,
    /// Mantém as camadas e os grupos; desligado, cada página vira uma camada e os grupos (exceto
    /// os de recorte) são desfeitos.
    pub preserve_layers: bool,
    /// Converte o texto em contornos (curvas) ao importar.
    pub text_to_outlines: bool,
    /// Lista nos avisos cada efeito ou recurso não importado (desligado: um resumo).
    pub report_unsupported: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self { scale: 1.0, preserve_layers: true, text_to_outlines: false, report_unsupported: true }
    }
}

/// Um documento CorelDRAW importado.
pub struct Imported {
    pub document: Document,
    pub info: CdrInfo,
    /// Avisos de compatibilidade: o que foi aproximado, convertido ou deixado de fora.
    pub warnings: Vec<String>,
    /// Famílias de fonte que o texto do arquivo usa (o engine avisa sobre as ausentes).
    pub fonts: Vec<String>,
    pub pages: usize,
    pub layers: usize,
    pub objects: usize,
}

/// Lê os bytes com o leitor externo, isolado.
pub(crate) fn parse(bytes: &[u8]) -> Result<(TdDocument, tracedraw_cdr::ParseReport)> {
    guarded("leitura do documento", || tracedraw_cdr::open_bytes(bytes, "CorelDRAW"))?.map_err(Error::from)
}

/// Objetos numa lista de formas, contando o conteúdo de grupos e recortes.
pub(crate) fn count_shapes(shapes: &[TdShape]) -> usize {
    shapes
        .iter()
        .map(|s| {
            1 + match &s.kind {
                ShapeKind::Group { children } => count_shapes(children),
                ShapeKind::ClipFrame { frame, contents } => count_shapes(std::slice::from_ref(frame)) + count_shapes(contents),
                _ => 0,
            }
        })
        .sum()
}

pub(crate) fn import(bytes: &[u8], title: &str, info: CdrInfo, opts: &ImportOptions) -> Result<Imported> {
    let scale = if opts.scale.is_finite() && opts.scale > 0.0 { opts.scale.clamp(1e-3, 1e3) } else { 1.0 };
    let (td, report) = parse(bytes)?;
    if td.pages.is_empty() {
        return Err(Error::Damaged("o documento não tem nenhuma página".into()));
    }
    let mut r = Reader {
        doc: Document::new(1.0, 1.0),
        td: &td,
        opts,
        k: PT_PER_MM * scale,
        notes: Notes::default(),
        fonts: BTreeSet::new(),
        layers: 0,
        objects: 0,
    };
    r.doc.layers.clear();
    r.doc.artboards.clear();
    r.doc.units = Unit::Millimeters;
    r.doc.title = title.to_string();
    let mut x = 0.0;
    let multi = td.pages.len() > 1;
    for (i, page) in td.pages.iter().enumerate() {
        let (w, h) = (page.size.width, page.size.height);
        if !(w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0) {
            r.notes.warn(format!("a página {} tem tamanho inválido e foi ignorada", i + 1));
            continue;
        }
        let (wp, hp) = (w * r.k, h * r.k);
        let rect = Rect::new(x, 0.0, x + wp, hp);
        r.doc.artboards.push(Artboard {
            id: r.doc.artboards.len() as u32 + 1,
            name: if page.name.trim().is_empty() { format!("Página {}", i + 1) } else { page.name.clone() },
            rect,
            show_center_mark: false,
            show_cross_hairs: false,
        });
        // Página do CorelDRAW (mm, Y para cima, origem embaixo à esquerda) → documento.
        let m = Affine::new([r.k, 0.0, 0.0, -r.k, x, hp]);
        let prefix = multi.then(|| format!("{}: ", r.doc.artboards.last().map_or(String::new(), |a| a.name.clone())));
        if let Some(bg) = &page.background {
            r.background(bg, rect, prefix.as_deref());
        }
        // Camadas mestras: desenhadas sob todas as páginas; aqui, sob a primeira.
        if i == 0 {
            for l in td.master.iter().filter(|l| !l.shapes.is_empty()) {
                if multi {
                    r.notes.warn("o conteúdo das camadas mestras foi colocado só na primeira página");
                }
                r.layer(l, m, Some("Mestre: "));
            }
        }
        let first = r.doc.layers.len();
        for l in &page.layers {
            r.layer(l, m, prefix.as_deref());
        }
        if !opts.preserve_layers && r.doc.layers.len() > first + 1 {
            r.merge_layers(first, prefix.as_deref().unwrap_or(""));
        }
        if !page.guides.is_empty() {
            r.notes.warn("guias das páginas não foram importadas");
        }
        x += wp + PAGE_GAP;
    }
    if r.doc.artboards.is_empty() {
        return Err(Error::Damaged("nenhuma página tem tamanho válido".into()));
    }
    if r.doc.layers.is_empty() {
        let id = r.doc.alloc_id();
        r.doc.layers.push(Arc::new(Node::layer(id, "Camada 1", LayerColor::Preset(0))));
    }
    if td.cmyk_profile.is_some() {
        r.notes.warn("o perfil CMYK incorporado ao arquivo não foi aplicado: os valores CMYK foram mantidos e exibidos pelo perfil ativo");
    }
    if !td.inks.is_empty() {
        r.notes.warn("cores spot (tintas nomeadas) foram convertidas na sua aproximação CMYK de processo");
    }
    if !td.fonts.is_empty() {
        r.notes.warn("fontes incorporadas ao arquivo não foram instaladas; o texto usa as fontes do sistema");
    }
    let mut warnings: Vec<String> = Vec::new();
    if r.objects == 0 && (!report.warnings.is_empty() || report.skipped_objects > 0) {
        warnings.push("nenhum objeto pôde ser lido: o arquivo parece danificado ou usa registros que o leitor não conhece".into());
    }
    if report.skipped_objects > 0 {
        warnings.push(format!("{} objeto(s) que o leitor CDR não reconhece foram ignorados", report.skipped_objects));
    }
    if opts.report_unsupported {
        warnings.extend(report.warnings.iter().map(|w| format!("Leitor CDR: {w}")));
        warnings.extend(r.notes.list);
    } else if !r.notes.list.is_empty() || !report.warnings.is_empty() {
        warnings.push(format!(
            "{} recurso(s) do arquivo foram aproximados ou não importados (ative \"Relatar efeitos não suportados\" para ver a lista)",
            r.notes.list.len() + report.warnings.len()
        ));
    }
    let mut doc = r.doc;
    doc.fix_next_id();
    doc.limit_depth_noting(&mut warnings);
    let layers = doc.layers.len();
    Ok(Imported { pages: doc.artboards.len(), layers, objects: r.objects, fonts: r.fonts.into_iter().collect(), document: doc, info, warnings })
}

/// Avisos sem repetição.
#[derive(Default)]
struct Notes {
    list: Vec<String>,
}

impl Notes {
    fn warn(&mut self, w: impl Into<String>) {
        let w = w.into();
        if !self.list.contains(&w) && self.list.len() < 200 {
            self.list.push(w);
        }
    }
}

struct Reader<'a> {
    doc: Document,
    td: &'a TdDocument,
    opts: &'a ImportOptions,
    /// Pontos do documento por milímetro do arquivo (com a escala pedida).
    k: f64,
    notes: Notes,
    fonts: BTreeSet<String>,
    layers: u8,
    objects: usize,
}

fn finite_affine(a: Affine) -> bool {
    a.as_coeffs().iter().all(|v| v.is_finite())
}

impl Reader<'_> {
    fn id(&mut self) -> NodeId {
        self.doc.alloc_id()
    }

    fn new_layer(&mut self, name: &str, children: Vec<Arc<Node>>) -> Node {
        let id = self.id();
        let mut n = Node::layer(id, name, LayerColor::Preset(self.layers % 9));
        self.layers = self.layers.wrapping_add(1);
        if let Some(c) = n.children_mut() {
            *c = children;
        }
        n
    }

    fn layer(&mut self, l: &TdLayer, m: Affine, prefix: Option<&str>) {
        let children: Vec<Arc<Node>> = l.shapes.iter().filter_map(|s| self.shape(s, m, 0)).map(Arc::new).collect();
        let name = format!("{}{}", prefix.unwrap_or(""), if l.name.trim().is_empty() { "Camada" } else { l.name.trim() });
        let mut n = self.new_layer(&name, children);
        n.visible = l.visible;
        n.locked = l.locked;
        if let NodeKind::Layer { printable, .. } = &mut n.kind {
            *printable = l.printable;
        }
        self.doc.layers.push(Arc::new(n));
    }

    /// Junta as camadas de `first` em diante numa só, desfazendo grupos simples.
    fn merge_layers(&mut self, first: usize, prefix: &str) {
        let merged: Vec<Arc<Node>> = self.doc.layers.drain(first..).flat_map(|l| l.children().cloned().unwrap_or_default()).collect();
        let mut flat = Vec::new();
        for n in merged {
            ungroup(n, &mut flat);
        }
        let l = self.new_layer(&format!("{prefix}Camada 1"), flat);
        self.doc.layers.push(Arc::new(l));
    }

    /// O fundo de uma página: um retângulo travado numa camada própria, sob a arte.
    fn background(&mut self, fill: &Fill, rect: Rect, prefix: Option<&str>) {
        let Some((paint, _)) = self.fill_paint(fill, rect) else { return };
        let mut n = Node::path(self.id(), PathData::from_bezpath(&rect.to_path(0.1)), Appearance::default());
        n.appearance = Appearance { items: vec![AppearanceItem::Fill(FillLayer::new(paint))], ..Appearance::default() }.into();
        n.name = Some("Fundo da página".into());
        let mut l = self.new_layer(&format!("{}Fundo da página", prefix.unwrap_or("")), vec![Arc::new(n)]);
        l.locked = true;
        self.doc.layers.push(Arc::new(l));
    }

    /// Uma forma (e sua subárvore) como arte do VectorCraft; `m` leva as coordenadas de página
    /// da forma ao documento.
    fn shape(&mut self, s: &TdShape, m: Affine, depth: usize) -> Option<Node> {
        if self.objects >= MAX_OBJECTS {
            self.notes.warn(format!("o arquivo tem mais de {MAX_OBJECTS} objetos: o restante foi ignorado"));
            return None;
        }
        if depth > MAX_DEPTH {
            self.notes.warn("grupos aninhados demais foram ignorados");
            return None;
        }
        if !finite_affine(s.transform) {
            self.notes.warn("objetos com transformação inválida foram ignorados");
            return None;
        }
        self.objects += 1;
        self.note_losses(s);
        let xf = m * s.transform;
        let mut out = match &s.kind {
            ShapeKind::Rect { .. } | ShapeKind::Ellipse { .. } | ShapeKind::Polygon { .. } | ShapeKind::Path { .. } => {
                let bp = xf * s.local_path();
                if !bp.elements().iter().all(el_finite) {
                    self.notes.warn("curvas com coordenadas inválidas foram ignoradas");
                    return None;
                }
                let appearance = self.appearance(s, bp.bounding_box(), xf);
                let mut n = Node::path(self.id(), PathData::from_bezpath(&bp), appearance);
                if let NodeKind::Path { rule, .. } = &mut n.kind {
                    // O CorelDRAW preenche curvas combinadas pela regra par-ímpar.
                    *rule = FillRule::EvenOdd;
                }
                n
            }
            ShapeKind::Group { children } => {
                let kids: Vec<Arc<Node>> = children.iter().filter_map(|c| self.shape(c, xf, depth + 1)).map(Arc::new).collect();
                if kids.is_empty() {
                    return None;
                }
                Node::group(self.id(), kids)
            }
            ShapeKind::ClipFrame { frame, contents } => {
                let clip_path = xf * frame.transform * frame.local_path();
                let mut clip = Node::path(self.id(), PathData::from_bezpath(&clip_path), self.appearance(frame, clip_path.bounding_box(), xf));
                if let NodeKind::Path { clipping, .. } = &mut clip.kind {
                    *clipping = true;
                }
                let mut kids = vec![Arc::new(clip)];
                kids.extend(contents.iter().filter_map(|c| self.shape(c, xf, depth + 1)).map(Arc::new));
                Node::new(self.id(), NodeKind::Group { children: kids, clip: true })
            }
            ShapeKind::Table(_) | ShapeKind::SymbolInstance { .. } => {
                if matches!(s.kind, ShapeKind::Table(_)) {
                    self.notes.warn("tabelas foram importadas como grupos de retângulos e textos");
                } else {
                    self.notes.warn("instâncias de símbolo foram importadas como grupos independentes");
                }
                let parts = s.expand(&self.td.symbols);
                let kids: Vec<Arc<Node>> = parts.iter().filter_map(|c| self.shape(c, xf, depth + 1)).map(Arc::new).collect();
                if kids.is_empty() {
                    return None;
                }
                Node::group(self.id(), kids)
            }
            ShapeKind::Text { spans, origin, frame, align, on_path, .. } => {
                if on_path.is_some() {
                    self.notes.warn("texto ajustado a caminho foi importado como texto comum, sem o caminho");
                }
                let text = self.text(s, spans, *origin, frame.map(|f| (f.width, f.height)), *align, xf)?;
                let n = Node::new(self.id(), NodeKind::Text(Box::new(text)));
                if self.opts.text_to_outlines {
                    match vectorcraft_effects::outline_text(&n) {
                        Some(o) => o,
                        None => {
                            self.notes.warn("um texto não pôde ser convertido em contornos e ficou editável");
                            n
                        }
                    }
                } else {
                    n
                }
            }
            ShapeKind::Bitmap { rect, width_px, height_px, png, .. } => self.bitmap(*rect, *width_px, *height_px, png, xf)?,
        };
        out.name = s.name.clone().filter(|n| !n.trim().is_empty());
        out.visible = s.visible;
        out.locked = s.locked;
        out.opacity = if s.opacity.is_finite() { s.opacity.clamp(0.0, 1.0) as f32 } else { 1.0 };
        Some(out)
    }

    fn note_losses(&mut self, s: &TdShape) {
        if s.shadow.is_some() {
            self.notes.warn("sombras projetadas não foram importadas");
        }
        if !s.effects.is_empty() || !s.parked_effects.is_empty() {
            self.notes.warn("efeitos dinâmicos do CorelDRAW (mistura, extrusão, contorno, envelope...) não foram importados como efeitos; vale a forma resultante gravada no arquivo");
        }
        if s.opacity_mask.is_some() {
            self.notes.warn("máscaras de opacidade não foram importadas");
        }
        if s.raster_fx.is_some() {
            self.notes.warn("efeitos de bitmap foram importados já aplicados à imagem (não editáveis)");
        }
        if s.overprint_fill || s.overprint_outline {
            self.notes.warn("sobreimpressão de preenchimento/contorno não foi importada");
        }
    }

    fn appearance(&mut self, s: &TdShape, bounds: Rect, xf: Affine) -> Appearance {
        let mut items = Vec::new();
        let mut above = Vec::new();
        for f in std::iter::once(&s.fill).chain(&s.extra_fills) {
            if let Some((paint, opacity)) = self.fill_paint(f, bounds) {
                let mut l = FillLayer::new(paint);
                l.opacity = opacity;
                items.push(AppearanceItem::Fill(l));
            }
        }
        for st in s.stroke.iter().chain(&s.extra_strokes) {
            let Some(l) = self.stroke(st, bounds, xf) else { continue };
            if st.behind_fill {
                items.insert(0, AppearanceItem::Stroke(l));
            } else {
                above.push(AppearanceItem::Stroke(l));
            }
        }
        items.extend(above);
        Appearance { items, ..Appearance::default() }
    }

    fn color(&mut self, c: TdColor) -> Color {
        let f = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        match c {
            TdColor::Rgb { r, g, b } => Color::Rgb { r: f(r), g: f(g), b: f(b) },
            TdColor::Cmyk { c, m, y, k } => Color::Cmyk { c: f(c), m: f(m), y: f(y), k: f(k) },
            TdColor::Gray { v } => Color::Gray { k: 1.0 - f(v) },
            TdColor::Lab { l, a, b } if l.is_finite() && a.is_finite() && b.is_finite() => Color::Lab { l, a, b },
            TdColor::Spot { tint, c, m, y, k, .. } => {
                let t = f(tint);
                Color::Cmyk { c: f(c) * t, m: f(m) * t, y: f(y) * t, k: f(k) * t }
            }
            TdColor::Registration => {
                self.notes.warn("a cor de registro foi convertida em CMYK 100/100/100/100");
                Color::Cmyk { c: 1.0, m: 1.0, y: 1.0, k: 1.0 }
            }
            other => {
                // HSB, HLS, YIQ e Lab inválido: pela conversão de tela.
                let [r, g, b] = other.to_rgb_f32();
                Color::Rgb { r: f(r), g: f(g), b: f(b) }
            }
        }
    }

    /// Um preenchimento como tinta do VectorCraft e a opacidade dela; `bounds` (documento) posiciona
    /// gradientes.
    fn fill_paint(&mut self, fill: &Fill, bounds: Rect) -> Option<(Paint, f32)> {
        match fill {
            Fill::None => None,
            Fill::Solid(c) => Some((Paint::solid(self.color(*c)), 1.0)),
            Fill::Fountain(f) => {
                let f = f.to_plain(64);
                if f.stops.len() < 2 {
                    return f.stops.first().map(|s| (Paint::solid(self.color(s.color)), 1.0));
                }
                let mut stops: Vec<GradientStop> = f
                    .stops
                    .iter()
                    .map(|s| GradientStop::new(if s.pos.is_finite() { s.pos.clamp(0.0, 1.0) as f32 } else { 0.0 }, self.color(s.color)))
                    .collect();
                stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
                let kind = match f.kind {
                    FountainKind::Linear => GradientKind::Linear,
                    FountainKind::Radial => GradientKind::Radial,
                    FountainKind::Conical | FountainKind::Square | FountainKind::Shaped => {
                        self.notes.warn("gradientes cônicos, quadrados e em forma foram importados como radiais");
                        GradientKind::Radial
                    }
                };
                let c = bounds.center();
                let (hw, hh) = (bounds.width() / 2.0, bounds.height() / 2.0);
                let pad = if f.edge_pad.is_finite() { f.edge_pad.clamp(0.0, 0.49) } else { 0.0 };
                let geom = if kind == GradientKind::Linear {
                    let a = if f.angle.is_finite() { f.angle.to_radians() } else { 0.0 };
                    // Ângulo anti-horário com Y para cima → direção no documento (Y para baixo).
                    let dir = Vec2::new(a.cos(), -a.sin());
                    let half = (hw * dir.x).abs() + (hh * dir.y).abs();
                    let span = half * (1.0 - 2.0 * pad);
                    GradientGeom { start: c - dir * span, end: c + dir * span, aspect: 1.0, focal: None }
                } else {
                    let off = Vec2::new(
                        if f.offset.x.is_finite() { f.offset.x.clamp(-1.0, 1.0) } else { 0.0 },
                        if f.offset.y.is_finite() { -f.offset.y.clamp(-1.0, 1.0) } else { 0.0 },
                    );
                    let centre = Point::new(c.x + off.x * hw, c.y + off.y * hh);
                    let r = hw.hypot(hh) * (1.0 - 2.0 * pad);
                    GradientGeom { start: centre, end: centre + Vec2::new(r.max(1e-6), 0.0), aspect: 1.0, focal: None }
                };
                let mut gp = GradientPaint::new(Gradient { kind, stops });
                gp.angle = if kind == GradientKind::Linear && f.angle.is_finite() { f.angle } else { 0.0 };
                gp.geom = Some(geom);
                Some((Paint::Gradient(Box::new(gp)), 1.0))
            }
            Fill::Pattern(_) | Fill::Texture(_) => {
                self.notes.warn("preenchimentos de padrão e de textura foram substituídos por cinza 50% (não há equivalente editável)");
                Some((Paint::solid(Color::Gray { k: 0.5 }), 1.0))
            }
            Fill::Mesh(mesh) => {
                self.notes.warn("preenchimentos de malha foram substituídos pela cor média dos nós");
                let n = mesh.nodes.len().max(1) as f32;
                let sum = mesh.nodes.iter().map(|p| self.color(p.color).to_rgb()).fold([0.0f32; 3], |a, c| [a[0] + c[0], a[1] + c[1], a[2] + c[2]]);
                Some((Paint::solid(Color::Rgb { r: sum[0] / n, g: sum[1] / n, b: sum[2] / n }), 1.0))
            }
        }
    }

    fn stroke(&mut self, st: &Stroke, bounds: Rect, xf: Affine) -> Option<StrokeLayer> {
        let paint = match st.fill.as_deref() {
            Some(f) => self.fill_paint(f, bounds).map(|(p, _)| p),
            None => Some(Paint::solid(self.color(st.color))),
        }?;
        // A espessura escala com o objeto quando assim marcada (o CorelDRAW guarda a do objeto
        // sem transformação).
        let scale = if st.scale_with_object { xf.determinant().abs().sqrt() / self.k } else { 1.0 };
        let w = st.width * self.k * if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let width = if w.is_finite() && w > 0.0 { w } else { HAIRLINE_PT };
        let mut l = StrokeLayer::new(paint, width);
        l.cap = match st.cap {
            TdCap::Butt => LineCap::Butt,
            TdCap::Round => LineCap::Round,
            TdCap::Square => LineCap::Square,
        };
        l.join = match st.join {
            TdJoin::Miter => LineJoin::Miter,
            TdJoin::Round => LineJoin::Round,
            TdJoin::Bevel => LineJoin::Bevel,
        };
        let dash: Vec<f64> = st.dash.iter().filter(|d| d.is_finite() && **d >= 0.0).map(|d| d * width).collect();
        if !dash.is_empty() && dash.iter().any(|d| *d > 0.0) {
            l.dash = Some(Dash { pattern: dash, offset: 0.0, align_corners: false });
        }
        if st.start_arrow != Arrowhead::None || st.end_arrow != Arrowhead::None {
            self.notes.warn("pontas de seta dos contornos não foram importadas");
        }
        if !st.widths.is_empty() {
            self.notes.warn("contornos de espessura variável foram importados com espessura uniforme");
        }
        if (st.stretch - 1.0).abs() > 1e-6 && st.stretch.is_finite() {
            self.notes.warn("contornos caligráficos (pena esticada) foram importados com pena redonda");
        }
        if st.painterly.is_some() {
            self.notes.warn("contornos artísticos (pincel) foram importados como linhas simples");
        }
        Some(l)
    }

    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        s: &TdShape,
        spans: &[TextSpan],
        origin: Point,
        frame: Option<(f64, f64)>,
        align: TextAlign,
        xf: Affine,
    ) -> Option<TextObject> {
        if !(origin.x.is_finite() && origin.y.is_finite()) {
            return None;
        }
        // Espaço do texto em pt com Y para baixo → mm locais (Y para cima) → documento. A escala
        // uniforme da transformação passa para o corpo, para os tamanhos lerem em pontos.
        let m = xf * Affine::translate(origin.to_vec2()) * Affine::new([1.0 / PT_PER_MM, 0.0, 0.0, -1.0 / PT_PER_MM, 0.0, 0.0]);
        let k = m.determinant().abs().sqrt();
        if !(k.is_finite() && k > 1e-9) {
            return None;
        }
        let mut obj = TextObject::point(Point::ZERO, "", CharStyle::default());
        obj.xf = m * Affine::scale(1.0 / k);
        obj.runs = spans
            .iter()
            .filter(|sp| !sp.text.is_empty())
            .map(|sp| {
                let mut style = CharStyle::default();
                if sp.font_family.trim().is_empty() {
                    self.notes.warn("texto sem nome de fonte usa a fonte padrão");
                } else {
                    style.font_family = sp.font_family.trim().to_string();
                    self.fonts.insert(style.font_family.clone());
                }
                style.font_style = match (sp.bold, sp.italic) {
                    (true, true) => "Bold Italic",
                    (true, false) => "Bold",
                    (false, true) => "Italic",
                    (false, false) => "Regular",
                }
                .into();
                let size = if sp.size_pt.is_finite() && sp.size_pt > 0.0 { sp.size_pt } else { 12.0 };
                style.size = size * k;
                if sp.tracking_pct.is_finite() {
                    // Porcentagem do espaço → milésimos de eme.
                    style.tracking = sp.tracking_pct * 10.0;
                }
                if sp.baseline_shift_pt.is_finite() {
                    style.baseline_shift = sp.baseline_shift_pt * k;
                }
                if sp.angle_deg.abs() > 1e-6 || sp.offset_x_pct.abs() > 1e-6 {
                    self.notes.warn("rotação e deslocamento de caracteres individuais não foram importados");
                }
                style.underline = sp.underline;
                style.strikethrough = sp.strikethrough;
                let fill = sp.fill.as_ref().unwrap_or(&s.fill);
                style.fill = self.fill_paint(fill, Rect::ZERO).map_or(Paint::None, |(p, _)| p);
                if matches!(style.fill, Paint::Gradient(_)) {
                    // Gradiente sem caixa conhecida: a cor do primeiro ponto.
                    style.fill = match fill {
                        Fill::Fountain(f) => f.stops.first().map_or(Paint::None, |st| Paint::solid(self.color(st.color))),
                        _ => style.fill.clone(),
                    };
                    self.notes.warn("textos com preenchimento gradiente foram importados com a primeira cor do gradiente");
                }
                if let Some(st) = &s.stroke {
                    style.stroke = Paint::solid(self.color(st.color));
                    style.stroke_width = (st.width * PT_PER_MM).max(0.0);
                }
                TextRun::new(sp.text.replace(['\r', '\u{2029}', '\u{2028}'], "\n"), style)
            })
            .collect();
        if obj.runs.is_empty() {
            return None;
        }
        obj.para.justify = match align {
            TextAlign::Left => Justify::Left,
            TextAlign::Center => Justify::Center,
            TextAlign::Right => Justify::Right,
            TextAlign::Justify => Justify::JustifyLeft,
        };
        if let Some((w, h)) = frame.filter(|(w, h)| w.is_finite() && h.is_finite() && *w > 0.0 && *h > 0.0) {
            // O quadro do texto de parágrafo pende do canto superior esquerdo (a origem).
            let r = Rect::new(0.0, 0.0, w * PT_PER_MM * k, h * PT_PER_MM * k);
            obj.kind = TextKind::Area { frame: shapes::rectangle(r) };
        }
        Some(obj)
    }

    fn bitmap(&mut self, rect: Rect, w: u32, h: u32, png: &[u8], xf: Affine) -> Option<Node> {
        let ok = (|| {
            let mut reader = image::ImageReader::new(std::io::Cursor::new(png)).with_guessed_format().ok()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(MAX_IMAGE_SIDE);
            limits.max_image_height = Some(MAX_IMAGE_SIDE);
            reader.limits(limits);
            reader.into_dimensions().ok()
        })();
        let Some((pw, ph)) = ok.filter(|(a, b)| *a > 0 && *b > 0) else {
            self.notes.warn("uma imagem incorporada que não pôde ser lida foi ignorada");
            return None;
        };
        if w != pw || h != ph {
            self.notes.warn("imagens cujas dimensões declaradas diferem dos pixels foram ajustadas aos pixels");
        }
        if !(rect.width().is_finite() && rect.height().is_finite()) || rect.width() == 0.0 || rect.height() == 0.0 {
            return None;
        }
        // Pixel (0, 0) é o canto superior esquerdo: em mm locais, (x0, y1) com Y para cima.
        let px = Affine::new([rect.width() / f64::from(pw), 0.0, 0.0, -rect.height() / f64::from(ph), rect.x0, rect.y1]);
        let blob = ImageBlob::new("image/png", png.to_vec());
        let key = blob.content_key();
        self.doc.images.entry(key.clone()).or_insert(blob);
        let im = ImageObject { key, width: pw, height: ph, xf: xf * px, link: None, placement: Default::default() };
        Some(Node::new(self.id(), NodeKind::Image(Box::new(im))))
    }
}

fn el_finite(e: &PathEl) -> bool {
    let p = |q: &Point| q.x.is_finite() && q.y.is_finite();
    match e {
        PathEl::MoveTo(a) | PathEl::LineTo(a) => p(a),
        PathEl::QuadTo(a, b) => p(a) && p(b),
        PathEl::CurveTo(a, b, c) => p(a) && p(b) && p(c),
        PathEl::ClosePath => true,
    }
}

/// `n` com os grupos simples desfeitos (grupos de recorte ficam).
fn ungroup(n: Arc<Node>, out: &mut Vec<Arc<Node>>) {
    match &n.kind {
        NodeKind::Group { children, clip: false } if n.opacity >= 0.999 => {
            for c in children.clone() {
                ungroup(c, out);
            }
        }
        _ => out.push(n),
    }
}
