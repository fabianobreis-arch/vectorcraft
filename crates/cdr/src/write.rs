//! Exportação: arte do VectorCraft (pontos, Y para baixo) convertida no documento do
//! `tracedraw-cdr` (milímetros, Y para cima), gravada pelo gravador CDR dele e, por padrão, relida
//! para validar o arquivo antes de entregá-lo.
//!
//! Cada prancheta vira uma página; cada objeto de primeiro nível de uma camada vai para a página
//! cuja prancheta contém o centro dele. Efeitos ao vivo são aplicados antes (a forma que desenham);
//! o que o CDR não guarda vira aviso. Nada é rasterizado aqui: efeitos de raster só viram imagem
//! quando o usuário autoriza (o engine os achata antes de chamar [`export`]).

use std::sync::Arc;

use kurbo::{Affine, BezPath, PathEl, Point, Rect, Shape as _};
use tracedraw_core::document::{Layer as TdLayer, Page as TdPage, Shape as TdShape, ShapeKind, TextSpan};
use tracedraw_core::style::{Fountain, FountainKind, LineCap as TdCap, LineJoin as TdJoin, Stop};
use tracedraw_core::{Color as TdColor, Document as TdDocument, Fill, Stroke, TextAlign};
use vectorcraft_color::{BlendMode, Color, GradientKind, Paint};
use vectorcraft_doc::text::{Justify, TextKind, TextObject};
use vectorcraft_doc::{AppearanceItem, Document, ImageObject, LineCap, LineJoin, Node, NodeKind, StrokeAlign};
use vectorcraft_geom::{FillRule, PathData};
use vectorcraft_pathops::BoolOp;

pub use tracedraw_cdr::write::{Png, Previews};

use crate::read::{PT_PER_MM, count_shapes};
use crate::{Error, NEWEST_EXPORT_VERSION, OLDEST_EXPORT_VERSION, Result, VERIFIED_SINCE, guarded};

/// Mais aninhamento de símbolos e grupos percorrido.
const MAX_DEPTH: usize = 64;
/// Maior imagem gravada (pixels por lado).
const MAX_IMAGE_SIDE: u32 = 16_384;

/// Preferências de exportação.
#[derive(Clone, Debug)]
pub struct ExportOptions {
    /// Versão do formato (7 a 22).
    pub version: u16,
    /// Converte o texto em contornos; desligado, o texto fica editável (o padrão a partir da
    /// versão 16, a primeira cujo texto gravado foi verificado no CorelDRAW).
    pub text_to_outlines: bool,
    /// Relê o arquivo gerado e o compara com o que foi escrito.
    pub validate: bool,
    /// Miniaturas guardadas no pacote (versão 16 em diante; as caixas de diálogo do CorelDRAW as
    /// mostram).
    pub previews: Previews,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self { version: crate::DEFAULT_EXPORT_VERSION, text_to_outlines: false, validate: true, previews: Previews::default() }
    }
}

/// Um arquivo CDR gerado.
#[derive(Clone, Debug)]
pub struct Exported {
    pub bytes: Vec<u8>,
    pub version: u16,
    /// O que foi aproximado, convertido ou deixado de fora.
    pub warnings: Vec<String>,
    /// O relatório da validação pós-exportação (`None`: desligada).
    pub validation: Option<String>,
    pub pages: usize,
    pub objects: usize,
}

pub(crate) fn export(doc: &Document, opts: &ExportOptions) -> Result<Exported> {
    let version = opts.version;
    if !(OLDEST_EXPORT_VERSION..=NEWEST_EXPORT_VERSION).contains(&version) {
        return Err(Error::Export(format!("a versão {version} não é gravada; escolha de {OLDEST_EXPORT_VERSION} a {NEWEST_EXPORT_VERSION}")));
    }
    if doc.artboards.is_empty() {
        return Err(Error::Export("o documento não tem prancheta (cada prancheta vira uma página)".into()));
    }
    let baked = vectorcraft_effects::bake_document(doc);
    let doc = baked.as_ref().unwrap_or(doc);
    let mut w = Writer {
        doc,
        notes: Vec::new(),
        td: TdDocument::new(doc.title.clone(), kurbo::Size::new(1.0, 1.0)),
        outline_text: opts.text_to_outlines,
        in_clip: 0,
    };
    if version < VERIFIED_SINCE {
        w.warn(format!(
            "a versão {version} é experimental: o gravador segue os arquivos de amostra dessa versão, mas a abertura no CorelDRAW não foi verificada"
        ));
        if !opts.text_to_outlines && has_text(doc) {
            w.warn("texto editável antes da versão 16 não foi verificado no CorelDRAW; se o texto não aparecer, exporte com \"Converter texto em contornos\"");
        }
    }
    let td = w.document()?;
    let pages = td.pages.len();
    let objects: usize = td.all_layers().map(|l| count_shapes(&l.shapes)).sum();
    let previews = if version >= 16 {
        opts.previews.clone()
    } else {
        Previews { thumbnail: None, pages: opts.previews.pages.iter().take(1).cloned().collect() }
    };
    let bytes = guarded("gravação CDR", || tracedraw_cdr::write::document_to_cdr_version_with(&td, version, &previews))?;
    let mut warnings = w.notes;
    let validation = if opts.validate {
        Some(validate_written(&bytes, version, &td).map_err(Error::Validation)?)
    } else {
        warnings.push("a validação pós-exportação estava desligada: o arquivo não foi conferido".into());
        None
    };
    Ok(Exported { bytes, version, warnings, validation, pages, objects })
}

/// Relê `bytes` e confere versão, páginas, tamanhos de página e contagem de objetos contra `td`.
fn validate_written(bytes: &[u8], version: u16, td: &TdDocument) -> std::result::Result<String, String> {
    let info = crate::detect_version(bytes).map_err(|e| format!("assinatura: {e}"))?;
    if info.version != version {
        return Err(format!("o arquivo declara a versão {} em vez de {version}", info.version));
    }
    let (back, report) = crate::read::parse(bytes).map_err(|e| format!("releitura: {e}"))?;
    if back.pages.len() != td.pages.len() {
        return Err(format!("{} página(s) gravada(s), {} relida(s)", td.pages.len(), back.pages.len()));
    }
    for (i, (a, b)) in td.pages.iter().zip(&back.pages).enumerate() {
        if (a.size.width - b.size.width).abs() > 0.05 || (a.size.height - b.size.height).abs() > 0.05 {
            return Err(format!(
                "a página {} foi gravada com {:.2} × {:.2} mm e relida com {:.2} × {:.2} mm",
                i + 1,
                a.size.width,
                a.size.height,
                b.size.width,
                b.size.height
            ));
        }
    }
    let written: usize = td.pages.iter().flat_map(|p| &p.layers).map(|l| count_shapes(&l.shapes)).sum();
    let read: usize = back.pages.iter().flat_map(|p| &p.layers).map(|l| count_shapes(&l.shapes)).sum();
    if report.skipped_objects > 0 || read < written {
        return Err(format!("{written} objeto(s) gravado(s), {read} relido(s) ({} ignorado(s) pelo leitor)", report.skipped_objects));
    }
    Ok(format!(
        "validado: {} {} ({}), {} página(s), {written} objeto(s) relidos sem erro",
        info.product,
        if version >= VERIFIED_SINCE { "— versão verificada no CorelDRAW pelo projeto de origem" } else { "— versão experimental" },
        info.container.label(),
        back.pages.len()
    ))
}

fn has_text(doc: &Document) -> bool {
    let mut any = false;
    doc.walk(|n| any |= matches!(n.kind, NodeKind::Text(_)));
    any
}

struct Writer<'a> {
    doc: &'a Document,
    notes: Vec<String>,
    td: TdDocument,
    outline_text: bool,
    /// Grupos de recorte em volta do nó atual.
    in_clip: u32,
}

fn mm(v: f64) -> f64 {
    v / PT_PER_MM
}

fn finite(r: Rect) -> bool {
    [r.x0, r.y0, r.x1, r.y1].iter().all(|v| v.is_finite())
}

impl Writer<'_> {
    fn warn(&mut self, w: impl Into<String>) {
        let w = w.into();
        if !self.notes.contains(&w) {
            self.notes.push(w);
        }
    }

    fn new_shape(&mut self, kind: ShapeKind) -> TdShape {
        let id = self.td.ids_mut().shape();
        TdShape::new(id, kind)
    }

    fn document(&mut self) -> Result<TdDocument> {
        let doc = self.doc;
        let boards: Vec<Rect> = doc.artboards.iter().map(|a| a.rect.abs()).collect();
        if boards.iter().any(|r| !finite(*r) || r.width() <= 0.0 || r.height() <= 0.0) {
            return Err(Error::Export("uma prancheta tem tamanho inválido".into()));
        }
        // Documento → página i (mm, Y para cima, origem embaixo à esquerda).
        let to_page: Vec<Affine> = boards.iter().map(|r| Affine::new([1.0 / PT_PER_MM, 0.0, 0.0, -1.0 / PT_PER_MM, -mm(r.x0), mm(r.y1)])).collect();
        let mut pages: Vec<TdPage> = Vec::new();
        for (i, a) in doc.artboards.iter().enumerate() {
            let r = boards.get(i).copied().unwrap_or(a.rect);
            let id = self.td.ids_mut().page();
            pages.push(TdPage {
                id,
                name: if a.name.trim().is_empty() { format!("Página {}", i + 1) } else { a.name.clone() },
                size: kurbo::Size::new(mm(r.width()), mm(r.height())),
                layers: Vec::new(),
                guides: Vec::new(),
                background: None,
            });
        }
        if !doc.guides.is_empty() {
            self.warn("guias não são exportadas");
        }
        for layer in &doc.layers {
            if layer.is_template() {
                self.warn("camadas de modelo (template) não são exportadas");
                continue;
            }
            let (printable, name) = match &layer.kind {
                NodeKind::Layer { printable, .. } => (*printable, layer.name.clone().unwrap_or_else(|| "Camada".into())),
                _ => (true, "Camada".into()),
            };
            // Cada página recebe a sua parte da camada.
            let mut per_page: Vec<Vec<TdShape>> = vec![Vec::new(); pages.len()];
            for child in layer.children().into_iter().flatten() {
                let page = self.page_of(child, &boards);
                let Some(m) = to_page.get(page).copied() else { continue };
                let shapes = self.node(child, m, 1.0, 0);
                if let Some(v) = per_page.get_mut(page) {
                    v.extend(shapes);
                }
            }
            for (p, shapes) in pages.iter_mut().zip(per_page) {
                let id = self.td.ids_mut().layer();
                let mut l = TdLayer::new(id, name.clone());
                l.visible = layer.visible;
                l.locked = layer.locked;
                l.printable = printable;
                l.shapes = shapes;
                p.layers.push(l);
            }
            if (layer.opacity - 1.0).abs() > 1e-3 {
                self.warn("a opacidade de camadas foi aplicada aos objetos delas");
            }
        }
        for p in &mut pages {
            if p.layers.is_empty() {
                let id = self.td.ids_mut().layer();
                p.layers.push(TdLayer::new(id, "Camada 1"));
            }
        }
        let mut td = std::mem::replace(&mut self.td, TdDocument::new("", kurbo::Size::new(1.0, 1.0)));
        td.pages = pages;
        td.title = doc.title.clone();
        td.metadata.author = doc.metadata.author.clone();
        Ok(td)
    }

    /// A página de um objeto: a prancheta que contém o centro dele (senão a de maior sobreposição,
    /// senão a primeira).
    fn page_of(&self, n: &Node, boards: &[Rect]) -> usize {
        let Some(b) = n.geometric_bounds().filter(|b| finite(*b)) else { return 0 };
        let c = b.center();
        if let Some(i) = boards.iter().position(|r| r.contains(c)) {
            return i;
        }
        boards
            .iter()
            .enumerate()
            .map(|(i, r)| (i, r.intersect(b).area().max(0.0)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|(_, a)| *a > 0.0)
            .map_or(0, |(i, _)| i)
    }

    fn note_losses(&mut self, n: &Node) {
        if n.blend != BlendMode::Normal {
            self.warn("modos de mesclagem não existem no formato e foram gravados como Normal");
        }
        if n.extras().mask.as_ref().is_some_and(|m| !m.disabled) {
            self.warn("máscaras de opacidade não são exportadas (o objeto é gravado sem a máscara)");
        }
        let raster = |fx: &[vectorcraft_doc::Effect]| fx.iter().any(|e| e.visible && vectorcraft_effects::is_raster(&e.id));
        if raster(&n.appearance.effects) || n.appearance.items.iter().any(|i| raster(i.effects())) {
            self.warn("efeitos de raster (sombras, brilhos, desfoques) foram deixados de fora; para mantê-los, autorize a rasterização nas opções de exportação");
        }
    }

    /// `n` como formas da página (`m`: documento → página); `opacity`: a dos contêineres acima.
    fn node(&mut self, n: &Node, m: Affine, opacity: f32, depth: usize) -> Vec<TdShape> {
        if depth > MAX_DEPTH {
            self.warn("arte aninhada demais foi deixada de fora");
            return vec![];
        }
        self.note_losses(n);
        let alpha = opacity * n.opacity.clamp(0.0, 1.0);
        let mut out: Vec<TdShape> = match &n.kind {
            NodeKind::Layer { children, clip, .. } | NodeKind::Group { children, clip } => {
                if n.is_layer() {
                    self.warn("subcamadas foram exportadas como grupos");
                }
                self.container(children, *clip, m, alpha, depth)
            }
            NodeKind::Path { path, guide, .. } => {
                if *guide {
                    return vec![];
                }
                self.path(n, &path.to_bezpath(), m, alpha).into_iter().collect()
            }
            NodeKind::Compound { children, .. } => {
                let mut bp = BezPath::new();
                for p in children.iter().filter_map(|c| c.path_data()) {
                    bp.extend(p.to_bezpath());
                }
                self.path(n, &bp, m, alpha).into_iter().collect()
            }
            NodeKind::Text(t) => {
                // Texto dentro de recorte é recortado como curvas; o gravador só grava texto
                // artístico (uma linha por parágrafo), então o de área, cuja quebra de linha se
                // perderia, vai como contornos.
                let editable = !self.outline_text && self.in_clip == 0 && !t.vertical && matches!(t.kind, TextKind::Point);
                if !editable && !self.outline_text {
                    if matches!(t.kind, TextKind::Area { .. }) && self.in_clip == 0 {
                        self.warn("texto de área (parágrafo) foi convertido em contornos: o formato gravado só guarda texto artístico, e as quebras de linha se perderiam");
                    } else if self.in_clip > 0 {
                        self.warn("texto dentro de grupos de recorte foi convertido em contornos para ser recortado");
                    } else {
                        self.warn("texto vertical e texto em caminho foram convertidos em contornos");
                    }
                }
                match editable.then(|| self.text(t, m, alpha)).flatten() {
                    Some(s) => vec![s],
                    None => match vectorcraft_effects::outline_text(n) {
                        Some(o) => self.node(&o, m, opacity, depth + 1),
                        None => {
                            self.warn("um texto que não pôde ser convertido foi deixado de fora (fonte ausente?)");
                            vec![]
                        }
                    },
                }
            }
            NodeKind::Image(im) => self.image(im, m, alpha).into_iter().collect(),
            NodeKind::SymbolInstance { symbol, xf } => {
                let Some(sym) = self.doc.symbols.iter().find(|s| &s.name == symbol) else { return vec![] };
                self.warn("instâncias de símbolo foram exportadas como grupos independentes");
                let mut art = (*sym.art).clone();
                art.transform(*xf, false);
                let kids = self.node(&art, m, alpha, depth + 1);
                self.group(kids)
            }
            NodeKind::Blend { .. }
            | NodeKind::Envelope { .. }
            | NodeKind::Mesh(_)
            | NodeKind::Repeat(_)
            | NodeKind::PlacedDocument(_)
            | NodeKind::Intertwine { .. } => {
                self.warn("mesclas, envelopes, malhas, repetições e documentos inseridos foram exportados como a arte que desenham (não editáveis como efeito)");
                let g = vectorcraft_effects::expand_live_deep(Some(self.doc), n);
                let kids: Vec<TdShape> = g.children().into_iter().flatten().flat_map(|c| self.node(c, m, alpha, depth + 1)).collect();
                self.group(kids)
            }
        };
        let name = n.name.clone().filter(|s| !s.trim().is_empty());
        for s in &mut out {
            if s.name.is_none() {
                s.name = name.clone();
            }
            s.visible &= n.visible;
            s.locked |= n.locked;
        }
        out
    }

    fn group(&mut self, kids: Vec<TdShape>) -> Vec<TdShape> {
        match kids.len() {
            0 => vec![],
            1 => kids,
            _ => vec![self.new_shape(ShapeKind::Group { children: kids })],
        }
    }

    fn container(&mut self, children: &[Arc<Node>], clip: bool, m: Affine, alpha: f32, depth: usize) -> Vec<TdShape> {
        let Some((first, rest)) = children.split_first().filter(|_| clip) else {
            let kids: Vec<TdShape> = children.iter().flat_map(|c| self.node(c, m, alpha, depth + 1)).collect();
            return self.group(kids);
        };
        // Grupo de recorte: o primeiro filho recorta os demais. O gravador não grava PowerClip de
        // vetores, então o recorte é aplicado à geometria (interseção booleana, que segue
        // editável); bitmaps recortados levam o caminho de corte nativo do formato.
        let Some((outline, rule)) = vectorcraft_effects::clip_outline(first) else { return vec![] };
        let clip_path = PathData::from_bezpath(&(m * outline));
        self.warn("grupos de recorte foram exportados com o recorte aplicado à geometria (contornos recortados viram áreas preenchidas); bitmaps recortados levam caminho de corte");
        let mut out = Vec::new();
        let frame_paint = first.path_data().map(|p| {
            let bp = m * p.to_bezpath();
            let b = bp.bounding_box();
            (bp, self.paints(first, alpha, b))
        });
        if let Some((bp, (fill, _, opacity))) = &frame_paint
            && !matches!(fill, Fill::None)
        {
            let mut f = self.new_shape(ShapeKind::Path { closed: true, path: bp.clone() });
            f.fill = fill.clone();
            f.opacity = f64::from(*opacity);
            out.push(f);
        }
        self.in_clip += 1;
        let contents: Vec<TdShape> = rest.iter().flat_map(|c| self.node(c, m, alpha, depth + 1)).collect();
        self.in_clip -= 1;
        for s in contents {
            let clipped = self.clip_shape(s, &clip_path, rule);
            out.extend(clipped);
        }
        if let Some((bp, (_, Some(stroke), opacity))) = frame_paint {
            let mut f = self.new_shape(ShapeKind::Path { closed: true, path: bp });
            f.stroke = Some(stroke);
            f.opacity = f64::from(opacity);
            out.push(f);
        }
        self.group(out)
    }

    /// `s` (coordenadas de página) recortado por `clip`.
    fn clip_shape(&mut self, mut s: TdShape, clip: &PathData, rule: FillRule) -> Vec<TdShape> {
        let inside = |p: &BezPath| {
            let r = vectorcraft_pathops::boolean(&PathData::from_bezpath(p), FillRule::EvenOdd, clip, rule, BoolOp::Intersect);
            (!r.is_empty()).then(|| r.to_bezpath())
        };
        match &mut s.kind {
            ShapeKind::Group { children } => {
                let mut kids = Vec::new();
                for c in std::mem::take(children) {
                    let clipped = self.clip_shape(c, clip, rule);
                    kids.extend(clipped);
                }
                if kids.is_empty() {
                    return vec![];
                }
                if let ShapeKind::Group { children } = &mut s.kind {
                    *children = kids;
                }
                vec![s]
            }
            ShapeKind::Path { path, .. } => {
                let source = s.transform * path.clone();
                let mut out = Vec::new();
                if !matches!(s.fill, Fill::None)
                    && let Some(p) = inside(&source)
                {
                    let mut f = self.new_shape(ShapeKind::Path { closed: true, path: p });
                    f.fill = s.fill.clone();
                    f.opacity = s.opacity;
                    f.name = s.name.clone();
                    f.visible = s.visible;
                    f.locked = s.locked;
                    out.push(f);
                }
                if let Some(st) = s.stroke.clone() {
                    if !st.dash.is_empty() {
                        self.warn("contornos tracejados dentro de recortes foram gravados contínuos");
                    }
                    let cap = match st.cap {
                        TdCap::Butt => vectorcraft_pathops::Cap::Butt,
                        TdCap::Round => vectorcraft_pathops::Cap::Round,
                        TdCap::Square => vectorcraft_pathops::Cap::Square,
                    };
                    let join = match st.join {
                        TdJoin::Miter => vectorcraft_pathops::Join::Miter,
                        TdJoin::Round => vectorcraft_pathops::Join::Round,
                        TdJoin::Bevel => vectorcraft_pathops::Join::Bevel,
                    };
                    let region = vectorcraft_pathops::outline_stroke(&PathData::from_bezpath(&source), st.width, cap, join, 4.0).to_bezpath();
                    if let Some(p) = inside(&region) {
                        let mut f = self.new_shape(ShapeKind::Path { closed: true, path: p });
                        f.fill = Fill::Solid(st.color);
                        f.opacity = s.opacity;
                        f.visible = s.visible;
                        f.locked = s.locked;
                        if st.behind_fill {
                            out.insert(0, f);
                        } else {
                            out.push(f);
                        }
                    }
                }
                out
            }
            ShapeKind::Bitmap { .. } => {
                let frame = self.new_shape(ShapeKind::Path { closed: true, path: clip.to_bezpath() });
                vec![self.new_shape(ShapeKind::ClipFrame { frame: Box::new(frame), contents: vec![s] })]
            }
            ShapeKind::ClipFrame { frame, .. } => {
                // Um bitmap já recortado: os dois caminhos de corte se cruzam.
                if let ShapeKind::Path { path, .. } = &mut frame.kind {
                    let p = frame.transform * path.clone();
                    match inside(&p) {
                        Some(q) => {
                            *path = q;
                            frame.transform = Affine::IDENTITY;
                        }
                        None => return vec![],
                    }
                }
                vec![s]
            }
            _ => {
                self.warn("um objeto dentro de um recorte foi gravado sem o recorte");
                vec![s]
            }
        }
    }

    fn path(&mut self, n: &Node, bp: &BezPath, m: Affine, alpha: f32) -> Option<TdShape> {
        if bp.elements().is_empty() {
            return None;
        }
        let page = m * bp.clone();
        if !page.elements().iter().all(el_finite) {
            self.warn("caminhos com coordenadas inválidas foram deixados de fora");
            return None;
        }
        let closed = page.elements().iter().any(|e| matches!(e, PathEl::ClosePath));
        let bounds = page.bounding_box();
        let (fill, stroke, opacity) = self.paints(n, alpha, bounds);
        let mut s = self.new_shape(ShapeKind::Path { path: page, closed });
        s.fill = fill;
        s.stroke = stroke;
        s.opacity = f64::from(opacity);
        Some(s)
    }

    /// O primeiro preenchimento e o primeiro contorno visíveis de `n` (o CDR guarda um de cada) e
    /// a opacidade uniforme do objeto.
    fn paints(&mut self, n: &Node, alpha: f32, bounds: Rect) -> (Fill, Option<Stroke>, f32) {
        let mut fill = None;
        let mut stroke: Option<Stroke> = None;
        let mut fill_opacity = 1.0f32;
        let mut stroke_first = false;
        let visible: Vec<&AppearanceItem> = n.appearance.items.iter().filter(|i| i.visible()).collect();
        let fills = visible.iter().filter(|i| matches!(i, AppearanceItem::Fill(_))).count();
        let strokes = visible.iter().filter(|i| matches!(i, AppearanceItem::Stroke(_))).count();
        if fills > 1 || strokes > 1 {
            self.warn("objetos com vários preenchimentos ou contornos foram gravados só com o primeiro de cada");
        }
        for item in visible {
            match item {
                AppearanceItem::Fill(f) if fill.is_none() => {
                    if f.blend != BlendMode::Normal {
                        self.warn("modos de mesclagem não existem no formato e foram gravados como Normal");
                    }
                    fill = Some(self.fill(&f.paint, bounds));
                    fill_opacity = f.opacity.clamp(0.0, 1.0);
                }
                AppearanceItem::Stroke(st) if stroke.is_none() && st.width.is_finite() && st.width > 0.0 => {
                    stroke_first = fill.is_none();
                    stroke = self.stroke(st, bounds);
                    if (st.opacity - fill_opacity).abs() > 0.01 && fill.is_some() {
                        self.warn("preenchimento e contorno com opacidades diferentes foram gravados com a opacidade do preenchimento");
                    }
                    if fill.is_none() {
                        fill_opacity = st.opacity.clamp(0.0, 1.0);
                    }
                }
                _ => {}
            }
        }
        if let Some(s) = stroke.as_mut() {
            s.behind_fill = stroke_first && fill.is_some();
        }
        (fill.unwrap_or(Fill::None), stroke, (alpha * fill_opacity).clamp(0.0, 1.0))
    }

    fn color(&mut self, c: &Color) -> TdColor {
        match *c {
            Color::Rgb { r, g, b } => TdColor::Rgb { r, g, b },
            Color::Cmyk { c, m, y, k } => TdColor::Cmyk { c, m, y, k },
            Color::Gray { k } => TdColor::Gray { v: 1.0 - k },
            Color::Lab { .. } => {
                self.warn("cores Lab foram gravadas como RGB");
                let [r, g, b] = c.to_rgb();
                TdColor::Rgb { r, g, b }
            }
        }
    }

    fn fill(&mut self, p: &Paint, bounds: Rect) -> Fill {
        match p {
            Paint::None => Fill::None,
            Paint::Solid { color, .. } => Fill::Solid(self.color(color)),
            Paint::Gradient(g) => {
                let stops: Vec<Stop> =
                    g.gradient.stops.iter().map(|s| Stop { pos: f64::from(s.offset.clamp(0.0, 1.0)), color: self.color(&s.color) }).collect();
                if g.gradient.stops.iter().any(|s| s.opacity < 0.999) {
                    self.warn("a transparência dos pontos de gradiente não existe no formato e foi descartada");
                }
                if g.gradient.stops.iter().any(|s| (s.midpoint - 0.5).abs() > 0.01) {
                    self.warn("pontos médios de gradiente foram gravados no meio");
                }
                if stops.is_empty() {
                    return Fill::None;
                }
                let kind = match g.gradient.kind {
                    GradientKind::Linear => FountainKind::Linear,
                    GradientKind::Radial => FountainKind::Radial,
                    GradientKind::Freeform => {
                        self.warn("gradientes de forma livre foram gravados como lineares com as mesmas cores");
                        FountainKind::Linear
                    }
                };
                let mut f = Fountain { kind, stops, ..Fountain::default() };
                match g.geom {
                    Some(geom) => {
                        // Geometria no documento (Y para baixo) → ângulo anti-horário Y para cima.
                        let d = geom.end - geom.start;
                        if kind == FountainKind::Linear && d.hypot() > 1e-9 {
                            f.angle = (-d.y).atan2(d.x).to_degrees();
                        }
                        if kind == FountainKind::Radial && finite(bounds) && bounds.width() > 0.0 && bounds.height() > 0.0 {
                            let c = bounds.center();
                            f.offset = Point::new(
                                ((geom.start.x - c.x) / (bounds.width() / 2.0)).clamp(-1.0, 1.0),
                                (-(geom.start.y - c.y) / (bounds.height() / 2.0)).clamp(-1.0, 1.0),
                            );
                            if (geom.aspect - 1.0).abs() > 0.01 {
                                self.warn("gradientes radiais elípticos foram gravados como circulares");
                            }
                        }
                    }
                    None => f.angle = if g.angle.is_finite() { g.angle } else { 0.0 },
                }
                Fill::Fountain(f)
            }
            Paint::Pattern { pattern, .. } => {
                self.warn("preenchimentos de padrão foram gravados com a primeira cor sólida do padrão");
                let c = self.doc.pattern(pattern).and_then(|def| def.art.iter().find_map(|a| first_solid(a)));
                Fill::Solid(c.map_or(TdColor::Gray { v: 0.5 }, |c| self.color(&c)))
            }
        }
    }

    fn stroke(&mut self, st: &vectorcraft_doc::StrokeLayer, bounds: Rect) -> Option<Stroke> {
        let color = match &st.paint {
            Paint::None => return None,
            Paint::Solid { color, .. } => self.color(color),
            other => {
                // Contorno com gradiente: o CDR guarda um preenchimento de contorno, que o gravador
                // não escreve; vale a cor.
                self.warn("contornos com gradiente ou padrão foram gravados com uma cor sólida");
                match self.fill(other, bounds) {
                    Fill::Solid(c) => c,
                    Fill::Fountain(f) => f.stops.first().map_or(TdColor::BLACK, |s| s.color),
                    _ => TdColor::BLACK,
                }
            }
        };
        let width = mm(st.width);
        let mut s = Stroke::new(color, width);
        s.cap = match st.cap {
            LineCap::Butt => TdCap::Butt,
            LineCap::Round => TdCap::Round,
            LineCap::Square => TdCap::Square,
        };
        s.join = match st.join {
            LineJoin::Miter => TdJoin::Miter,
            LineJoin::Round => TdJoin::Round,
            LineJoin::Bevel => TdJoin::Bevel,
        };
        s.scale_with_object = false;
        if let Some(d) = st.dash.as_ref().filter(|d| d.is_dashed()) {
            // Traços em múltiplos da espessura.
            s.dash = d.pattern.iter().map(|v| if st.width > 0.0 { v / st.width } else { *v }).filter(|v| v.is_finite()).take(10).collect();
            if d.pattern.len() > 10 {
                self.warn("padrões de traço com mais de 10 elementos foram encurtados");
            }
        }
        if st.align != StrokeAlign::Center {
            self.warn("contornos alinhados por dentro ou por fora foram gravados centralizados");
        }
        if st.start_arrow.is_some() || st.end_arrow.is_some() {
            self.warn("pontas de seta não são exportadas");
        }
        if st.profile.is_some() {
            self.warn("perfis de espessura variável foram gravados com espessura uniforme");
        }
        if st.brush.is_some() {
            self.warn("pincéis foram gravados como contornos simples");
        }
        Some(s)
    }

    fn text(&mut self, t: &TextObject, m: Affine, alpha: f32) -> Option<TdShape> {
        // A escala uniforme da matriz do texto vai para o corpo (o quadro de um texto de parágrafo
        // escala com o objeto no CorelDRAW, mas as letras não).
        let s = t.xf.determinant().abs().sqrt();
        if !(s.is_finite() && s > 1e-9) {
            return None;
        }
        let (frame, offset) = match &t.kind {
            TextKind::Area { frame } => {
                let b = frame.to_bezpath().bounding_box();
                if frame.subpaths.len() != 1 || !finite(b) || b.width() <= 0.0 || b.height() <= 0.0 {
                    return None;
                }
                (Some(kurbo::Size::new(mm(b.width() * s), mm(b.height() * s))), b.origin().to_vec2())
            }
            _ => (None, kurbo::Vec2::ZERO),
        };
        // Inverso do que a importação faz: mm locais (Y para cima, origem do texto — o canto do
        // quadro, no texto de parágrafo — em (0, 0)) → espaço do texto (pt, Y para baixo) →
        // documento → página.
        let xf = t.xf * Affine::translate(offset) * Affine::scale(1.0 / s);
        let transform = m * xf * Affine::new([PT_PER_MM, 0.0, 0.0, -PT_PER_MM, 0.0, 0.0]);
        if !transform.as_coeffs().iter().all(|v| v.is_finite()) {
            return None;
        }
        let spans: Vec<TextSpan> = t
            .runs
            .iter()
            .filter(|r| !r.text.is_empty())
            .map(|r| {
                let st = &r.style;
                let size = if st.size.is_finite() && st.size > 0.0 { st.size } else { 12.0 };
                let mut sp = TextSpan::new(r.text.clone(), st.font_family.clone(), size * s);
                let style = st.font_style.to_ascii_lowercase();
                sp.bold = ["bold", "black", "heavy"].iter().any(|w| style.contains(w));
                sp.italic = style.contains("italic") || style.contains("oblique");
                sp.underline = st.underline;
                sp.strikethrough = st.strikethrough;
                if st.tracking.is_finite() {
                    sp.tracking_pct = st.tracking / 10.0;
                }
                if st.baseline_shift.is_finite() {
                    sp.baseline_shift_pt = st.baseline_shift * s;
                }
                sp.fill = match &st.fill {
                    Paint::None => Some(Fill::None),
                    p => Some(self.fill(p, Rect::ZERO)),
                };
                if (st.h_scale - 1.0).abs() > 1e-3 || (st.v_scale - 1.0).abs() > 1e-3 || st.rotation.abs() > 1e-6 {
                    self.warn("escala horizontal/vertical e rotação de caracteres não foram exportadas");
                }
                sp
            })
            .collect();
        if spans.is_empty() {
            return None;
        }
        if t.runs.iter().any(|r| r.inline.is_some()) {
            self.warn("gráficos embutidos no texto não foram exportados");
        }
        let align = justify(t.para.justify);
        let mut shape = self.new_shape(ShapeKind::Text { spans, origin: Point::ZERO, frame, align, para: Default::default(), on_path: None });
        shape.transform = transform;
        shape.opacity = f64::from(alpha);
        Some(shape)
    }

    fn image(&mut self, im: &ImageObject, m: Affine, alpha: f32) -> Option<TdShape> {
        let Some(blob) = self.doc.images.get(&im.key).filter(|b| !b.bytes.is_empty()) else {
            self.warn("imagens vinculadas sem os pixels disponíveis foram deixadas de fora");
            return None;
        };
        let decoded = (|| {
            let mut reader = image::ImageReader::new(std::io::Cursor::new(blob.bytes.as_slice())).with_guessed_format().ok()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(MAX_IMAGE_SIDE);
            limits.max_image_height = Some(MAX_IMAGE_SIDE);
            reader.limits(limits);
            reader.decode().ok()
        })();
        let Some(img) = decoded else {
            self.warn("uma imagem que não pôde ser decodificada foi deixada de fora");
            return None;
        };
        let (pw, ph) = (img.width(), img.height());
        let png = if blob.mime == "image/png" {
            blob.bytes.as_ref().clone()
        } else {
            let mut out = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
            out
        };
        if im.link.is_some() {
            self.warn("imagens vinculadas foram incorporadas ao arquivo CDR");
        }
        // Pixel (px, py) → documento é `im.xf` (na grade de im.width × im.height); em mm locais a
        // imagem ocupa (0, 0)–(pw, ph) com Y para cima.
        let grid = Affine::scale_non_uniform(f64::from(im.width.max(1)) / f64::from(pw.max(1)), f64::from(im.height.max(1)) / f64::from(ph.max(1)));
        let flip = Affine::new([1.0, 0.0, 0.0, -1.0, 0.0, f64::from(ph)]);
        let transform = m * im.xf * grid * flip;
        if !transform.as_coeffs().iter().all(|v| v.is_finite()) {
            return None;
        }
        let rect = Rect::new(0.0, 0.0, f64::from(pw), f64::from(ph));
        let mut s = self.new_shape(ShapeKind::Bitmap { rect, width_px: pw, height_px: ph, png, fx: None });
        s.transform = transform;
        s.opacity = f64::from(alpha);
        Some(s)
    }
}

fn justify(j: Justify) -> TextAlign {
    match j {
        Justify::Center | Justify::JustifyCenter => TextAlign::Center,
        Justify::Right | Justify::JustifyRight => TextAlign::Right,
        Justify::JustifyLeft => TextAlign::Justify,
        _ => TextAlign::Left,
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

/// A primeira cor sólida visível em `n` (a arte de um padrão).
fn first_solid(n: &Node) -> Option<Color> {
    let own = n.appearance.items.iter().find_map(|i| match i {
        AppearanceItem::Fill(f) if f.visible => f.paint.color(),
        _ => None,
    });
    own.or_else(|| n.children().into_iter().flatten().find_map(|c| first_solid(c)))
}
