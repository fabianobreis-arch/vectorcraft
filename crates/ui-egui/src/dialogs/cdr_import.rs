//! CorelDRAW Import Options: abrir (Arquivo › Abrir CDR, Arquivo › Abrir) ou importar (Arquivo ›
//! Importar CDR, Inserir) um documento `.cdr` mostra a versão, o contêiner e o conteúdo que o
//! leitor encontrou e pergunta as preferências: escala original, camadas e grupos, texto editável
//! ou em contornos, aviso de fontes ausentes e relatório do que não foi suportado.
//!
//! Campos: `mode` (`open` | `place`), `name`, `path?`, `product`, `version`, `container`,
//! `pages`, `objects`, `pageW`/`pageH` (mm), `readable`, `error?`, e as opções `cdr` de
//! `document.open`: `preserveScale`, `preserveLayers`, `textToOutlines`, `warnMissingFonts`,
//! `reportUnsupported`. `__place` guarda os outros parâmetros de `file.place`. Os bytes ficam em
//! [`crate::state::UiState::dialog_file`]. OK lembra as escolhas para o próximo arquivo; erros
//! mantêm o diálogo aberto.

use serde_json::{Value, json};
use vectorcraft_engine::cmd::fileio;

use super::DialogSpec;
use super::document_setup::check;
use super::import_pdf::{DialogFile, other_place_params, place_source, with_file};
use crate::state::Dialog;
use crate::{VectorcraftApp, io, widgets};

pub const KIND: &str = "cdrImport";

pub(super) const SPEC: DialogSpec = DialogSpec {
    heading: |d| if d.str("mode") == "place" { tl!("Import CorelDRAW") } else { tl!("CorelDRAW Import Options") }.into(),
    body,
    confirm,
    min_width: 460.0,
    max_width: Some(520.0),
    ..DialogSpec::FORM
};

/// As escolhas que o próximo arquivo herda.
const REMEMBERED: [&str; 5] = ["preserveScale", "preserveLayers", "textToOutlines", "warnMissingFonts", "reportUnsupported"];

/// Pontos por milímetro.
const PT_PER_MM: f64 = 72.0 / 25.4;

/// Abre (ou, com `place`, importa) `bytes` pelo diálogo quando são um documento CorelDRAW; false
/// quando não são (o carregador diz então por quê).
pub fn offer(app: &mut VectorcraftApp, name: &str, bytes: &[u8], path: Option<String>, place: Option<&Value>) -> bool {
    if fileio::detect(name, bytes).is_none_or(|f| f.id != "cdr") {
        return false;
    }
    let Ok(info) = vectorcraft_cdr::detect_version(bytes) else { return false };
    let v = vectorcraft_cdr::validate(bytes);
    app.ui.dialog_file = Some(DialogFile::new(bytes.to_vec()));
    let (w, h) = v.page_size.unwrap_or((0.0, 0.0));
    let mut fields = json!({
        "mode": if place.is_some() { "place" } else { "open" },
        "name": name,
        "product": info.product,
        "version": info.version,
        "container": info.container.label(),
        "note": info.note,
        "pages": v.pages,
        "objects": v.objects,
        "pageW": w,
        "pageH": h,
        "readable": v.ok(),
        "error": v.error,
        "preserveScale": true,
        "preserveLayers": true,
        "textToOutlines": false,
        "warnMissingFonts": true,
        "reportUnsupported": true,
    });
    if let Some(last) = app.ui.cdr_import.as_object() {
        for k in REMEMBERED {
            if let Some(v) = last.get(k).filter(|v| v.is_boolean()) {
                fields[k] = v.clone();
            }
        }
    }
    if let Some(p) = path {
        fields["path"] = json!(p);
    }
    if let Some(p) = place {
        fields["__place"] = other_place_params(p, &["cdr"]);
    }
    app.ui.dialog = Some(Dialog::new(KIND, fields));
    true
}

/// `file.place` com os parâmetros `p` pergunta antes (true) quando eles nomeiam um `.cdr` sem
/// opções `cdr`.
pub fn offer_place(app: &mut VectorcraftApp, p: &Value) -> bool {
    let named = p.get("path").or_else(|| p.get("name")).and_then(Value::as_str).map(fileio::extension);
    if p.get("cdr").is_some() || named.as_deref() != Some("cdr") {
        return false;
    }
    let Some((name, bytes, path)) = place_source(app, p, |ext| ext == "cdr") else { return false };
    offer(app, &name, &bytes, path, Some(p))
}

/// Arquivo › Abrir CDR…: escolhe um `.cdr` e o abre pelo diálogo.
pub fn pick_open(app: &mut VectorcraftApp) -> Result<Value, String> {
    let path = crate::picks::open(app, &crate::FilePick { filters: vec![("CorelDRAW", &["cdr"])], ..Default::default() }).ok_or("cancelled")?;
    io::open_reporting(app, &path)
}

/// Arquivo › Importar CDR…: escolhe um `.cdr` e o importa no documento ativo pelo diálogo.
pub fn pick_import(app: &mut VectorcraftApp) -> Result<Value, String> {
    if app.session.active().is_none() {
        return Err("no document".into());
    }
    let path = crate::picks::open(app, &crate::FilePick { filters: vec![("CorelDRAW", &["cdr"])], ..Default::default() }).ok_or("cancelled")?;
    crate::place::run(app, &json!({ "path": path }))
}

/// As opções `cdr` de `document.open` e `file.place` que os campos definem.
fn options(app: &VectorcraftApp, d: &Dialog) -> Value {
    let mut o = json!({
        "preserveScale": d.bool("preserveScale"),
        "preserveLayers": d.bool("preserveLayers"),
        "textToOutlines": d.bool("textToOutlines"),
        "warnMissingFonts": d.bool("warnMissingFonts"),
        "reportUnsupported": d.bool("reportUnsupported"),
    });
    // Sem a escala original, a primeira página cabe na prancheta ativa (ao importar).
    if !d.bool("preserveScale")
        && d.str("mode") == "place"
        && let Some(st) = app.session.active()
        && let Some(a) = st.doc.artboards.first()
    {
        let (pw, ph) = (d.f64("pageW", 0.0) * PT_PER_MM, d.f64("pageH", 0.0) * PT_PER_MM);
        if pw > 0.0 && ph > 0.0 {
            o["scale"] = json!((a.rect.width() / pw).min(a.rect.height() / ph).clamp(0.001, 1000.0));
        }
    }
    o
}

fn confirm(app: &mut VectorcraftApp, d: &Dialog) -> Result<Value, String> {
    if !d.bool("readable") {
        return Err(d.fields.get("error").and_then(Value::as_str).unwrap_or("o documento CorelDRAW não pôde ser lido").to_string());
    }
    let file = app.ui.dialog_file.clone().ok_or("the drawing is no longer loaded")?;
    let cdr = options(app, d);
    app.ui.cdr_import = Value::Object(REMEMBERED.iter().map(|k| (k.to_string(), cdr[*k].clone())).collect());
    let path = d.fields.get("path").and_then(Value::as_str).map(str::to_string);
    let r = if d.str("mode") == "place" {
        let mut p = with_file(d.fields.get("__place"), d, &file);
        p["cdr"] = cdr;
        crate::place::run(app, &p).map(|_| ())
    } else {
        // Documentos grandes abrem numa thread, com o progresso na barra de status.
        let params = json!({ "cdr": cdr });
        match path.as_deref().map(|p| crate::opening::start_with(app, p, file.bytes.clone(), params.clone())) {
            Some(Ok(_)) => Ok(()),
            _ => io::open_document(app, &d.str("name"), &file.bytes, path.clone(), &params).map(|_| ()),
        }
    };
    match r {
        Ok(()) => {
            if app.ui.dialog.as_ref().is_some_and(|x| x.kind == KIND) {
                app.ui.dialog = None;
            }
            app.status(format!("{}: {}", tl!("CorelDRAW document read"), d.str("name")));
            Ok(Value::Null)
        }
        Err(e) => {
            app.ui.dialog = Some(d.clone());
            Err(e)
        }
    }
}

fn body(_app: &mut VectorcraftApp, ui: &mut egui::Ui, d: &mut Dialog) -> bool {
    widgets::subheader(ui, tl!("File"));
    widgets::dim_label(ui, &format!("{} · {} {} · {}", d.str("product"), tl!("format version"), d.f64("version", 0.0), d.str("container")));
    if d.bool("readable") {
        widgets::dim_label(
            ui,
            &format!(
                "{} {} · {} {} · {:.1} × {:.1} mm",
                d.f64("pages", 0.0),
                tl!("page(s)"),
                d.f64("objects", 0.0),
                tl!("object(s)"),
                d.f64("pageW", 0.0),
                d.f64("pageH", 0.0)
            ),
        );
    } else {
        super::form::caption(
            ui,
            &format!("{} {}", tl!("This file can't be read:"), d.fields.get("error").and_then(Value::as_str).unwrap_or_default()),
        );
    }
    if let Some(n) = d.fields.get("note").and_then(Value::as_str) {
        super::form::caption(ui, n);
    }
    ui.add_space(10.0);
    widgets::subheader(ui, tl!("Scale and Structure"));
    let place = d.str("mode") == "place";
    if widgets::check(ui, tl!("Preserve Original Document Scale"), d.bool("preserveScale") || !place, place) {
        d.fields.insert("preserveScale".into(), json!(!d.bool("preserveScale")));
    }
    if place && !d.bool("preserveScale") {
        super::form::caption(ui, tl!("The first page is scaled to fit the first artboard."));
    }
    check(ui, d, "preserveLayers", tl!("Preserve Layers and Groups"));
    ui.add_space(10.0);
    widgets::subheader(ui, tl!("Text"));
    let outlines = d.bool("textToOutlines");
    if widgets::radio(ui, tl!("Keep Text Editable When Possible"), !outlines, true) {
        d.fields.insert("textToOutlines".into(), json!(false));
    }
    if widgets::radio(ui, tl!("Convert Text to Outlines"), outlines, true) {
        d.fields.insert("textToOutlines".into(), json!(true));
    }
    check(ui, d, "warnMissingFonts", tl!("Warn About Missing Fonts"));
    ui.add_space(10.0);
    widgets::subheader(ui, tl!("Compatibility"));
    check(ui, d, "reportUnsupported", tl!("Report Unsupported Effects"));
    super::form::caption(ui, tl!("Editable vectors are never turned into images. What can't be kept is listed in the import notes."));
    false
}
