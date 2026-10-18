# vectorcraft-cdr

Subsistema de compatibilidade com o CorelDRAW (`.cdr`) do VectorCraft.

- **Leitura**: versões 3 a 26 (RIFF até a 13; ZIP com `content/riffData.cdr` na 14 e 15; ZIP com
  `content/root.dat` da 16 em diante).
- **Gravação**: versões 7 a 22. Da 16 (CorelDRAW X6) em diante os arquivos gravados foram abertos
  no CorelDRAW pelos testes do projeto de origem; as anteriores são marcadas como experimentais.
- **Validação**: toda exportação relê o arquivo gerado e confere versão, páginas, tamanhos e
  contagem de objetos; um arquivo que não passa não é gravado.

O formato binário é tratado pelos crates `tracedraw-cdr`/`tracedraw-core` do
[TraceDraw](https://github.com/igtoth/tracedraw) (MIT OR Apache-2.0, clean-room, fixados num
commit). Este crate faz a conversão entre os modelos de documento, os limites para arquivos não
confiáveis, o isolamento de falhas e os diagnósticos. Matriz de compatibilidade:
[`docs/cdr-compatibility.md`](../../docs/cdr-compatibility.md).

| Interface pedida | Aqui |
|---|---|
| `detect_cdr_version(path)` | `detect_version(&bytes) -> Result<CdrInfo>` |
| `validate_cdr_file(path)` | `validate(&bytes) -> Validation` |
| `open_cdr(path)` | `import(&bytes, title, &ImportOptions) -> Result<Imported>` (engine: `document.open`) |
| `import_cdr(path, document)` | o mesmo `import`, colocado no documento por `file.place` |
| `export_cdr(document, path, version)` | `export(&doc, &ExportOptions { version, .. }) -> Result<Exported>` (engine: `document.export {format: cdr}`) |
| relatório de capacidades | `matrix::FEATURES`, `matrix::to_json()`, `export_versions()` (engine: `document.cdrCapabilities`) |
