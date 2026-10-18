//! Subsistema de compatibilidade com o CorelDRAW (`.cdr`).
//!
//! O formato CDR não tem especificação pública oficial. Em vez de um parser binário especulativo,
//! este crate integra o leitor e o gravador clean-room do projeto TraceDraw (`tracedraw-cdr`,
//! MIT OR Apache-2.0, fixado num commit), que cobre:
//!
//! - **RIFF** (versões 7 a 13; antes da 6 com coordenadas de 16 bits), com listas `cmpr`
//!   comprimidas por zlib;
//! - **ZIP** das versões 14 e 15 (`content/riffData.cdr`) e das versões 16 em diante
//!   (`content/root.dat`, `dataFileList.dat` e registros de redirecionamento de 16 bytes).
//!
//! O que fica aqui é a camada do VectorCraft: detecção e validação de assinatura e versão, limites
//! para arquivos não confiáveis, isolamento de falhas internas do leitor (nunca derruba o app),
//! conversão entre os dois modelos de documento (geometria, cores, transparência, camadas, grupos,
//! recortes, texto, imagens, páginas) e os diagnósticos: cada perda vira um aviso, nada é
//! rasterizado em silêncio, e a matriz de compatibilidade ([`matrix`]) diz o que é suportado.
//!
//! Interfaces (equivalentes às pedidas, adaptadas a bytes, como o resto do VectorCraft lê
//! arquivos):
//!
//! - [`detect_version`] / [`is_cdr`]: assinatura e versão;
//! - [`validate`]: o arquivo é lido inteiro, sem montar documento;
//! - [`import`]: um documento VectorCraft (abrir; o engine usa o mesmo para Importar/Colocar);
//! - [`export`]: grava a versão pedida e, por padrão, valida o arquivo gerado relendo-o.

#![forbid(unsafe_code)]

pub mod matrix;
mod read;
mod write;

#[cfg(test)]
mod tests;

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

pub use read::{ImportOptions, Imported};
pub use write::{ExportOptions, Exported, Png, Previews};

/// Maior arquivo lido (bytes): acima disso a leitura é recusada antes de começar.
pub const MAX_FILE_BYTES: usize = 1 << 30;
/// Versão mais antiga que o gravador produz.
pub const OLDEST_EXPORT_VERSION: u16 = tracedraw_cdr::write::OLDEST_VERSION;
/// Versão mais nova que o gravador produz.
pub const NEWEST_EXPORT_VERSION: u16 = tracedraw_cdr::write::NEWEST_VERSION;
/// A partir desta versão os arquivos gravados foram abertos com sucesso no CorelDRAW pelos
/// testes do projeto de origem (texto incluído). As anteriores são experimentais.
pub const VERIFIED_SINCE: u16 = tracedraw_cdr::write::CHECKED_SINCE;
/// Versão padrão da exportação: a mais antiga verificada, que abre na maior gama de versões do
/// CorelDRAW (X6 em diante).
pub const DEFAULT_EXPORT_VERSION: u16 = VERIFIED_SINCE;
/// Versão mais nova vista pelo leitor em arquivos reais; acima dela a leitura é tentada com aviso.
pub const NEWEST_KNOWN_VERSION: u16 = 26;

/// Por que um arquivo CDR não pôde ser lido ou gravado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A assinatura não é de um documento CorelDRAW.
    NotCdr,
    /// Um contêiner ou versão que o leitor não trata.
    Unsupported(String),
    /// Dados truncados, comprimidos de forma inválida ou incoerentes.
    Damaged(String),
    /// Maior que [`MAX_FILE_BYTES`].
    TooLarge(usize),
    /// O documento não pode ser exportado (sem prancheta, versão fora da faixa...).
    Export(String),
    /// O arquivo gerado não passou na validação pós-exportação: nada foi gravado.
    Validation(String),
    /// Falha interna do leitor ou do gravador (isolada: o aplicativo continua).
    Internal(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotCdr => {
                write!(f, "o arquivo não é um documento CorelDRAW (.cdr): assinatura RIFF/CDR ou pacote ZIP do CorelDRAW não encontrados")
            }
            Self::Unsupported(w) => write!(f, "documento CorelDRAW não suportado: {w}"),
            Self::Damaged(w) => write!(f, "o documento CorelDRAW está danificado ou incompleto: {w}"),
            Self::TooLarge(n) => write!(f, "o arquivo tem {} MB; o limite para documentos CorelDRAW é {} MB", n >> 20, MAX_FILE_BYTES >> 20),
            Self::Export(w) => write!(f, "não foi possível exportar para CorelDRAW: {w}"),
            Self::Validation(w) => write!(f, "o arquivo CDR gerado não passou na validação e não foi gravado: {w}"),
            Self::Internal(w) => write!(f, "falha interna ao processar o documento CorelDRAW ({w}); o arquivo pode estar corrompido"),
        }
    }
}

impl std::error::Error for Error {}

impl From<tracedraw_cdr::Error> for Error {
    fn from(e: tracedraw_cdr::Error) -> Self {
        use tracedraw_cdr::Error as E;
        match e {
            E::NotCdr | E::NotCmx => Self::NotCdr,
            E::UnsupportedContainer(w) => Self::Unsupported(w),
            E::Truncated(at) => Self::Damaged(format!("dados truncados no byte {at}")),
            E::Io(e) => Self::Damaged(e.to_string()),
            E::Zip(e) => Self::Damaged(format!("pacote ZIP inválido: {e}")),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Executa código do leitor/gravador externo isolando um eventual pânico (dados hostis podem
/// levar código de terceiros a um caminho não previsto): vira [`Error::Internal`], nunca um crash.
pub(crate) fn guarded<T>(what: &str, f: impl FnOnce() -> T) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|p| {
        let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
        Error::Internal(if msg.is_empty() { what.to_string() } else { format!("{what}: {msg}") })
    })
}

/// O contêiner de um arquivo CDR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    /// Arquivo RIFF (versões até a 13).
    Riff,
    /// Pacote ZIP com `content/riffData.cdr` (versões 14 e 15).
    ZipRiffData,
    /// Pacote ZIP com `content/root.dat` e fluxos externos (versão 16 em diante).
    ZipRootDat,
}

impl Container {
    pub fn label(self) -> &'static str {
        match self {
            Self::Riff => "RIFF",
            Self::ZipRiffData => "ZIP (content/riffData.cdr)",
            Self::ZipRootDat => "ZIP (content/root.dat)",
        }
    }
}

/// Assinatura, versão e contêiner de um arquivo CDR.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CdrInfo {
    /// Versão principal do formato (7 a 26...), como o tipo RIFF a codifica.
    pub version: u16,
    pub container: Container,
    /// O produto que grava essa versão ("CorelDRAW X6"...).
    pub product: String,
    /// O leitor trata esta versão (as anteriores à 6 e as posteriores a
    /// [`NEWEST_KNOWN_VERSION`] são lidas com ressalvas).
    pub supported: bool,
    /// Observação sobre o suporte desta versão.
    pub note: Option<String>,
}

/// O nome comercial do CorelDRAW que grava a versão `v` do formato.
pub fn product_name(v: u16) -> String {
    match v {
        13 => "CorelDRAW X3".into(),
        14 => "CorelDRAW X4".into(),
        15 => "CorelDRAW X5".into(),
        16 => "CorelDRAW X6".into(),
        17 => "CorelDRAW X7".into(),
        18 => "CorelDRAW X8".into(),
        19..=26 => format!("CorelDRAW {}", 2017 + u32::from(v - 19)),
        0 => "CorelDRAW (versão desconhecida)".into(),
        v => format!("CorelDRAW {v}"),
    }
}

/// Como uma versão de exportação é tratada.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportStatus {
    /// Arquivos desta versão abriram no CorelDRAW nos testes do projeto de origem.
    Verified,
    /// Gravada com os layouts dos arquivos de amostra, sem teste de abertura no CorelDRAW.
    Experimental,
}

/// Uma versão que a exportação oferece.
#[derive(Clone, Debug)]
pub struct ExportVersion {
    pub version: u16,
    pub product: String,
    pub container: Container,
    pub status: ExportStatus,
}

/// As versões que [`export`] grava, da mais antiga à mais nova.
pub fn export_versions() -> Vec<ExportVersion> {
    (OLDEST_EXPORT_VERSION..=NEWEST_EXPORT_VERSION)
        .map(|v| ExportVersion {
            version: v,
            product: product_name(v),
            container: container_of(v),
            status: if v >= VERIFIED_SINCE { ExportStatus::Verified } else { ExportStatus::Experimental },
        })
        .collect()
}

fn container_of(v: u16) -> Container {
    match v {
        ..=13 => Container::Riff,
        14 | 15 => Container::ZipRiffData,
        _ => Container::ZipRootDat,
    }
}

const ZIP_MAGIC: &[u8] = b"PK\x03\x04";

/// Os bytes têm cara de documento CorelDRAW? Barato: só a assinatura RIFF (`CDR?`/`cdr?`) ou, num
/// ZIP, o nome de um membro do pacote do CorelDRAW no diretório central (sem descomprimir nada).
pub fn is_cdr(bytes: &[u8]) -> bool {
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") {
        return bytes.get(8..11).is_some_and(|f| f.eq_ignore_ascii_case(b"cdr"));
    }
    bytes.starts_with(ZIP_MAGIC) && zip_names_cdr(bytes)
}

/// Algum nome de membro do ZIP é o fluxo RIFF de um pacote do CorelDRAW?
fn zip_names_cdr(bytes: &[u8]) -> bool {
    // Os nomes ficam nos cabeçalhos locais e no diretório central, no fim do arquivo.
    let head = bytes.get(..bytes.len().min(1 << 16)).unwrap_or_default();
    let tail = bytes.get(bytes.len().saturating_sub(1 << 22)..).unwrap_or_default();
    [b"content/riffData.cdr".as_slice(), b"content/riffdata.cdr", b"content/root.dat"]
        .iter()
        .any(|needle| [head, tail].iter().any(|part| part.windows(needle.len()).any(|w| w == *needle)))
}

/// A versão e o contêiner de um arquivo CDR. Erros descritivos para o que não é CDR ou não é lido.
pub fn detect_version(bytes: &[u8]) -> Result<CdrInfo> {
    if bytes.len() > MAX_FILE_BYTES {
        return Err(Error::TooLarge(bytes.len()));
    }
    if !is_cdr(bytes) {
        return Err(Error::NotCdr);
    }
    let c = guarded("detecção do contêiner", || tracedraw_cdr::container::detect(bytes))??;
    let container = match &c.kind {
        tracedraw_cdr::container::ContainerKind::Riff => Container::Riff,
        tracedraw_cdr::container::ContainerKind::Zip { riff_member, .. } if riff_member.ends_with("root.dat") => Container::ZipRootDat,
        tracedraw_cdr::container::ContainerKind::Zip { .. } => Container::ZipRiffData,
    };
    let version = c.version.0;
    let (supported, note) = match version {
        0 => (false, Some("a versão não pôde ser determinada; a leitura será tentada".to_string())),
        1..=5 => (true, Some("versão anterior à 6 (coordenadas de 16 bits): suporte parcial, pouco testado".to_string())),
        v if v > NEWEST_KNOWN_VERSION => {
            (true, Some(format!("versão {v} é mais nova que as testadas (até {NEWEST_KNOWN_VERSION}); a leitura será tentada")))
        }
        _ => (true, None),
    };
    Ok(CdrInfo { version, container, product: product_name(version), supported, note })
}

/// O resultado de [`validate`].
#[derive(Clone, Debug, Default)]
pub struct Validation {
    pub info: Option<CdrInfo>,
    /// O arquivo foi lido do começo ao fim.
    pub readable: bool,
    pub pages: usize,
    pub layers: usize,
    /// Objetos (formas, textos, imagens, grupos) em todas as páginas e camadas mestras.
    pub objects: usize,
    /// Registros de objeto que o leitor não entendeu e pulou.
    pub skipped: usize,
    pub warnings: Vec<String>,
    /// Tamanho da primeira página (mm).
    pub page_size: Option<(f64, f64)>,
    /// Por que o arquivo não pôde ser lido.
    pub error: Option<String>,
}

impl Validation {
    pub fn ok(&self) -> bool {
        self.readable && self.error.is_none()
    }
}

/// Lê o arquivo inteiro (assinatura, versão, árvore de blocos, objetos) sem criar documento do
/// VectorCraft: para conferir um arquivo antes de abrir e depois de exportar.
pub fn validate(bytes: &[u8]) -> Validation {
    let mut v = Validation::default();
    match detect_version(bytes) {
        Ok(info) => v.info = Some(info),
        Err(e) => {
            v.error = Some(e.to_string());
            return v;
        }
    }
    match read::parse(bytes) {
        Ok((doc, report)) => {
            v.readable = true;
            v.pages = doc.pages.len();
            v.layers = doc.all_layers().count();
            v.objects = doc.all_layers().map(|l| read::count_shapes(&l.shapes)).sum();
            v.skipped = report.skipped_objects;
            v.page_size = doc.pages.first().map(|p| (p.size.width, p.size.height));
            v.warnings = report.warnings;
        }
        Err(e) => v.error = Some(e.to_string()),
    }
    v
}

/// Lê um documento CorelDRAW como documento do VectorCraft (uma prancheta por página).
pub fn import(bytes: &[u8], title: &str, opts: &ImportOptions) -> Result<Imported> {
    let info = detect_version(bytes)?;
    read::import(bytes, title, info, opts)
}

/// Grava `doc` como CDR da versão `opts.version` e, com `opts.validate`, relê o arquivo gerado e o
/// compara com o que foi escrito; um arquivo que não passa é recusado ([`Error::Validation`]).
pub fn export(doc: &vectorcraft_doc::Document, opts: &ExportOptions) -> Result<Exported> {
    write::export(doc, opts)
}
