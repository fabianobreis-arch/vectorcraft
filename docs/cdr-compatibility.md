# Compatibilidade com CorelDRAW (.cdr)

Status: implementado localmente (branch `feat/cdr-suporte`). Gerado em parte de
`crates/cdr/src/matrix.rs` (o teste `compatibility_doc_matches_the_matrix` confere).

## Onde está na interface

- **Arquivo › Abrir…** — o filtro inclui `*.cdr` (o formato também é reconhecido pelo conteúdo).
  Mostra as *Opções de Importação do CorelDRAW*.
- **Arquivo › Colocar…** — importa um `.cdr` no documento aberto, pelo mesmo diálogo.
- **Arquivo › Exportar › Exportar Como CDR…** e **Arquivo › Exportar › Exportar Como… › CorelDRAW**
  — mostram as *Opções de Exportação do CorelDRAW*.

Preferências de importação: preservar a escala original; preservar camadas e grupos; manter texto
editável ou converter em contornos; avisar sobre fontes ausentes; relatar efeitos não suportados.

Preferências de exportação: versão de destino (com o status de cada uma); texto editável ou em
contornos; caminhos vetoriais sempre preservados (nunca rasterizados); autorização explícita para
rasterizar efeitos de raster; validação pós-exportação (ligada por padrão).

Comandos para automação/MCP: `document.cdrInfo`, `document.cdrCapabilities`,
`document.open {cdr: {...}}`, `file.place {cdr: {...}}`, `document.export {format: "cdr", version, textToOutlines, validate, rasterizeEffects}`.

## Versões

| Versão | Produto | Contêiner | Leitura | Gravação |
|---|---|---|---|---|
| 3–6 | CorelDRAW 3–6 | RIFF (16 bits antes da 6) | parcial | — |
| 7–13 | CorelDRAW 7–12, X3 | RIFF + `cmpr` (zlib) | sim | experimental |
| 14–15 | CorelDRAW X4, X5 | ZIP (`content/riffData.cdr`) | sim | experimental |
| 16–22 | CorelDRAW X6 … 2020 | ZIP (`content/root.dat`) | sim | **verificada** (padrão: 16) |
| 23–26 | CorelDRAW 2021 … 2024 | ZIP (`content/root.dat`) | sim | — (grave em 22, que essas versões abrem) |

"Verificada" quer dizer que arquivos gravados nessa versão abriram no CorelDRAW nos testes do
projeto de origem (TraceDraw). **Nesta máquina a abertura no CorelDRAW não foi testada**: a
validação local relê o arquivo com o leitor e compara com o que foi gravado.

## Matriz de recursos

<!-- matriz:inicio -->
| Recurso | Importação | Exportação |
|---|---|---|
| Caminhos vetoriais e curvas de Bézier | **Totalmente suportada** — Nós, curvas cúbicas e subcaminhos fechados preservados como caminhos editáveis (regra par-ímpar, como o CorelDRAW) | **Totalmente suportada** — Gravados como curvas do CDR com seus nós; o tipo de nó (cúspide/suave/simétrico) é deduzido das alças |
| Formas e primitivas geométricas | **Convertida com limitações** — Retângulos (com cantos), elipses, arcos, polígonos e estrelas viram caminhos editáveis com a mesma geometria; o parâmetro da primitiva se perde | **Convertida com limitações** — Formas ao vivo do VectorCraft são gravadas como curvas |
| Posicionamento e transformações | **Totalmente suportada** — Matriz de cada objeto aplicada à geometria; texto e imagens mantêm a matriz | **Totalmente suportada** — Coordenadas convertidas de pontos para unidades do CDR relativas à página; texto e imagens com matriz própria |
| Grupos e hierarquia | **Totalmente suportada** — Grupos aninhados preservados (até 64 níveis); tabelas e símbolos viram grupos | **Parcialmente suportada** — Grupos preservados; subcamadas viram grupos; símbolos, mesclas e repetições viram a arte que desenham |
| Camadas e ordem das camadas | **Totalmente suportada** — Nome, ordem, visibilidade, bloqueio e impressão; camadas mestras sob a primeira página | **Parcialmente suportada** — Nome, ordem, visibilidade, bloqueio e impressão; a opacidade da camada passa aos objetos; camadas de modelo ficam de fora |
| Conteúdo de texto e atributos de fonte | **Parcialmente suportada** — Texto artístico e de parágrafo editáveis: família, negrito/itálico, corpo, cor, sublinhado, tachado, alinhamento, espaçamento entre caracteres. Texto em caminho perde o caminho; entrelinha e recuos de parágrafo usam o padrão | **Parcialmente suportada** — Texto de ponto gravado como texto artístico editável (verificado no CorelDRAW a partir da versão 16) ou como contornos por opção; texto de área, vertical, em caminho ou dentro de recortes vira contornos |
| Cores de preenchimento e propriedades de contorno | **Totalmente suportada** — Cores RGB, CMYK, tons de cinza e Lab; contorno com espessura, terminações, junções e tracejado | **Parcialmente suportada** — Um preenchimento e um contorno por objeto (o primeiro de cada); alinhamento interno/externo, setas e perfis de largura se perdem; Lab vira RGB |
| Gradientes | **Convertida com limitações** — Lineares e radiais com seus pontos de cor; cônicos, quadrados e em forma viram radiais; repetição e espelhamento expandidos em pontos | **Convertida com limitações** — Lineares e radiais com ângulo e centro; opacidade e ponto médio dos pontos de cor se perdem; forma livre vira linear |
| Transparência | **Parcialmente suportada** — Transparência uniforme do objeto vira a opacidade dele; transparências gradientes não | **Parcialmente suportada** — Opacidade do objeto, do preenchimento e dos grupos combinada numa transparência uniforme por objeto |
| Caminhos de recorte e máscaras | **Parcialmente suportada** — PowerClip vira grupo de recorte editável; máscaras de opacidade não são importadas | **Convertida com limitações** — O recorte é aplicado à geometria (interseção booleana, editável; contornos recortados viram áreas preenchidas); bitmaps recortados levam caminho de corte nativo; máscaras de opacidade e modos de mesclagem não são gravados |
| Dimensões da página e unidades | **Totalmente suportada** — Tamanho físico exato (mm → pt), documento em milímetros | **Totalmente suportada** — Cada prancheta vira uma página do mesmo tamanho; conferido na validação |
| Múltiplas páginas | **Totalmente suportada** — Uma prancheta por página, lado a lado, com as camadas de cada página | **Totalmente suportada** — Uma página por prancheta; cada objeto vai para a prancheta que contém o centro dele |
| Bitmaps incorporados | **Totalmente suportada** — Pixels preservados (RGB, CMYK convertido, transparência) com posição e transformação | **Parcialmente suportada** — Imagens incorporadas em 24 bits; imagens vinculadas são incorporadas; a transparência alfa do bitmap pode se perder |
| Perfis de cor e CMYK | **Parcialmente suportada** — Valores CMYK preservados; o perfil ICC do arquivo não é aplicado (vale o perfil ativo); cores spot viram CMYK | **Parcialmente suportada** — Valores CMYK e RGB preservados; perfis ICC e cores spot não são gravados |
| Efeitos (sombras, mesclas, extrusões, lentes, envelopes) | **Não suportada** — Efeitos dinâmicos não são recriados; vale a forma gravada no arquivo; sombras ficam de fora (com aviso) | **Convertida com limitações** — Efeitos vetoriais ao vivo são aplicados e gravados como formas; efeitos de raster ficam de fora, salvo se a rasterização for autorizada |
| Preenchimentos de padrão, textura e malha | **Convertida com limitações** — Substituídos por uma cor sólida aproximada (com aviso) | **Convertida com limitações** — Padrões gravados com a primeira cor sólida deles; malhas de gradiente gravadas como a arte que desenham |
<!-- matriz:fim -->

## Limitações conhecidas do gravador

- Grupos de recorte de vetores não são gravados como PowerClip: o recorte é aplicado à geometria
  (interseção booleana; contornos recortados viram áreas preenchidas). Bitmaps recortados usam o
  caminho de corte nativo.
- Texto de área (parágrafo), vertical e em caminho é gravado como contornos.
- Um preenchimento e um contorno por objeto; transparência uniforme por objeto.
- Perfis ICC, cores spot, máscaras de opacidade e modos de mesclagem não são gravados.
