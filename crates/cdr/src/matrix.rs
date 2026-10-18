//! Matriz de compatibilidade CDR: o que cada recurso vira na importação e na exportação. É a fonte
//! do comando `document.cdrCapabilities`, da caixa de diálogo e de `docs/cdr-compatibility.md`.
//! Uma conversão com perdas nunca aparece como suporte total.

use serde::Serialize;

/// Nível de suporte de um recurso.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// Totalmente suportada: preservado e editável.
    Full,
    /// Parcialmente suportada: o essencial é preservado, alguns atributos se perdem.
    Partial,
    /// Convertida com limitações: vira outra representação editável (curvas, grupos, cor sólida).
    Converted,
    /// Não suportada: deixada de fora, com aviso.
    Unsupported,
}

impl Level {
    pub fn label(self) -> &'static str {
        match self {
            Self::Full => "Totalmente suportada",
            Self::Partial => "Parcialmente suportada",
            Self::Converted => "Convertida com limitações",
            Self::Unsupported => "Não suportada",
        }
    }
}

/// Um recurso e como ele é tratado em cada sentido.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Feature {
    pub feature: &'static str,
    pub import: Level,
    pub import_note: &'static str,
    pub export: Level,
    pub export_note: &'static str,
}

use Level::*;

/// A matriz, na ordem da lista de fidelidade gráfica.
pub const FEATURES: &[Feature] = &[
    Feature {
        feature: "Caminhos vetoriais e curvas de Bézier",
        import: Full,
        import_note: "Nós, curvas cúbicas e subcaminhos fechados preservados como caminhos editáveis (regra par-ímpar, como o CorelDRAW)",
        export: Full,
        export_note: "Gravados como curvas do CDR com seus nós; o tipo de nó (cúspide/suave/simétrico) é deduzido das alças",
    },
    Feature {
        feature: "Formas e primitivas geométricas",
        import: Converted,
        import_note: "Retângulos (com cantos), elipses, arcos, polígonos e estrelas viram caminhos editáveis com a mesma geometria; o parâmetro da primitiva se perde",
        export: Converted,
        export_note: "Formas ao vivo do VectorCraft são gravadas como curvas",
    },
    Feature {
        feature: "Posicionamento e transformações",
        import: Full,
        import_note: "Matriz de cada objeto aplicada à geometria; texto e imagens mantêm a matriz",
        export: Full,
        export_note: "Coordenadas convertidas de pontos para unidades do CDR relativas à página; texto e imagens com matriz própria",
    },
    Feature {
        feature: "Grupos e hierarquia",
        import: Full,
        import_note: "Grupos aninhados preservados (até 64 níveis); tabelas e símbolos viram grupos",
        export: Partial,
        export_note: "Grupos preservados; subcamadas viram grupos; símbolos, mesclas e repetições viram a arte que desenham",
    },
    Feature {
        feature: "Camadas e ordem das camadas",
        import: Full,
        import_note: "Nome, ordem, visibilidade, bloqueio e impressão; camadas mestras sob a primeira página",
        export: Partial,
        export_note: "Nome, ordem, visibilidade, bloqueio e impressão; a opacidade da camada passa aos objetos; camadas de modelo ficam de fora",
    },
    Feature {
        feature: "Conteúdo de texto e atributos de fonte",
        import: Partial,
        import_note: "Texto artístico e de parágrafo editáveis: família, negrito/itálico, corpo, cor, sublinhado, tachado, alinhamento, espaçamento entre caracteres. Texto em caminho perde o caminho; entrelinha e recuos de parágrafo usam o padrão",
        export: Partial,
        export_note: "Texto de ponto gravado como texto artístico editável (verificado no CorelDRAW a partir da versão 16) ou como contornos por opção; texto de área, vertical, em caminho ou dentro de recortes vira contornos",
    },
    Feature {
        feature: "Cores de preenchimento e propriedades de contorno",
        import: Full,
        import_note: "Cores RGB, CMYK, tons de cinza e Lab; contorno com espessura, terminações, junções e tracejado",
        export: Partial,
        export_note: "Um preenchimento e um contorno por objeto (o primeiro de cada); alinhamento interno/externo, setas e perfis de largura se perdem; Lab vira RGB",
    },
    Feature {
        feature: "Gradientes",
        import: Converted,
        import_note: "Lineares e radiais com seus pontos de cor; cônicos, quadrados e em forma viram radiais; repetição e espelhamento expandidos em pontos",
        export: Converted,
        export_note: "Lineares e radiais com ângulo e centro; opacidade e ponto médio dos pontos de cor se perdem; forma livre vira linear",
    },
    Feature {
        feature: "Transparência",
        import: Partial,
        import_note: "Transparência uniforme do objeto vira a opacidade dele; transparências gradientes não",
        export: Partial,
        export_note: "Opacidade do objeto, do preenchimento e dos grupos combinada numa transparência uniforme por objeto",
    },
    Feature {
        feature: "Caminhos de recorte e máscaras",
        import: Partial,
        import_note: "PowerClip vira grupo de recorte editável; máscaras de opacidade não são importadas",
        export: Converted,
        export_note: "O recorte é aplicado à geometria (interseção booleana, editável; contornos recortados viram áreas preenchidas); bitmaps recortados levam caminho de corte nativo; máscaras de opacidade e modos de mesclagem não são gravados",
    },
    Feature {
        feature: "Dimensões da página e unidades",
        import: Full,
        import_note: "Tamanho físico exato (mm → pt), documento em milímetros",
        export: Full,
        export_note: "Cada prancheta vira uma página do mesmo tamanho; conferido na validação",
    },
    Feature {
        feature: "Múltiplas páginas",
        import: Full,
        import_note: "Uma prancheta por página, lado a lado, com as camadas de cada página",
        export: Full,
        export_note: "Uma página por prancheta; cada objeto vai para a prancheta que contém o centro dele",
    },
    Feature {
        feature: "Bitmaps incorporados",
        import: Full,
        import_note: "Pixels preservados (RGB, CMYK convertido, transparência) com posição e transformação",
        export: Partial,
        export_note: "Imagens incorporadas em 24 bits; imagens vinculadas são incorporadas; a transparência alfa do bitmap pode se perder",
    },
    Feature {
        feature: "Perfis de cor e CMYK",
        import: Partial,
        import_note: "Valores CMYK preservados; o perfil ICC do arquivo não é aplicado (vale o perfil ativo); cores spot viram CMYK",
        export: Partial,
        export_note: "Valores CMYK e RGB preservados; perfis ICC e cores spot não são gravados",
    },
    Feature {
        feature: "Efeitos (sombras, mesclas, extrusões, lentes, envelopes)",
        import: Unsupported,
        import_note: "Efeitos dinâmicos não são recriados; vale a forma gravada no arquivo; sombras ficam de fora (com aviso)",
        export: Converted,
        export_note: "Efeitos vetoriais ao vivo são aplicados e gravados como formas; efeitos de raster ficam de fora, salvo se a rasterização for autorizada",
    },
    Feature {
        feature: "Preenchimentos de padrão, textura e malha",
        import: Converted,
        import_note: "Substituídos por uma cor sólida aproximada (com aviso)",
        export: Converted,
        export_note: "Padrões gravados com a primeira cor sólida deles; malhas de gradiente gravadas como a arte que desenham",
    },
];

/// Versões de leitura: o que o leitor trata.
pub const READ_NOTE: &str = "Leitura: versões 3 a 26 (RIFF até a 13; ZIP com riffData.cdr na 14 e 15; ZIP com root.dat da 16 em diante). Antes da 6 o suporte é parcial; arquivos CMX não são abertos por este comando.";

/// A matriz em Markdown (para documentação).
pub fn markdown() -> String {
    let mut s = String::from("| Recurso | Importação | Exportação |\n|---|---|---|\n");
    for f in FEATURES {
        s.push_str(&format!("| {} | **{}** — {} | **{}** — {} |\n", f.feature, f.import.label(), f.import_note, f.export.label(), f.export_note));
    }
    s
}

/// A matriz e as versões de exportação como JSON.
pub fn to_json() -> serde_json::Value {
    let versions: Vec<serde_json::Value> = crate::export_versions()
        .into_iter()
        .map(|v| {
            serde_json::json!({
                "version": v.version,
                "product": v.product,
                "container": v.container.label(),
                "status": match v.status {
                    crate::ExportStatus::Verified => "verified",
                    crate::ExportStatus::Experimental => "experimental",
                },
            })
        })
        .collect();
    serde_json::json!({
        "features": FEATURES,
        "read": READ_NOTE,
        "exportVersions": versions,
        "defaultExportVersion": crate::DEFAULT_EXPORT_VERSION,
        "verifiedSince": crate::VERIFIED_SINCE,
        "levels": ["full", "partial", "converted", "unsupported"],
    })
}
