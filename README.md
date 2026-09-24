# Travaso

**Un server MCP che travasa una memoria da un'intelligenza artificiale a un'altra.** Esempio: da Claude a ChatGPT.

Il nome viene dal travaso del vino: si versa da una botte all'altra finché la prima è vuota, e il fondo non si butta.

| | Cosa fa |
|---|---|
| **1. Conserva** | Al primo avvio fotografa la memoria in `archivio/`, che non viene mai modificato. |
| **2. Scarica** | Ogni consultazione consegna solo fatti nuovi. Quando l'IA li ha salvati nella propria memoria chiama `travaso_assorbi` e i fatti escono dal serbatoio. |
| **3. Svuota** | Quando ogni fatto è stato assorbito il serbatoio arriva a 0%: il travaso è finito. L'archivio resta consultabile e ripristinabile. |

## Perché un MCP e non una skill
MCP (Model Context Protocol) è lo standard aperto supportato da ChatGPT (Codex e connettori in modalità sviluppatore), Claude (Desktop e Code), Cursor, Gemini CLI, VS Code/Copilot e molti altri. Una skill è un file di istruzioni che non tutti i client leggono. Per questo Travaso è **un MCP**. `skill/SKILL.md` è il manuale d'uso facoltativo per i client che supportano le skill (Codex, Claude).

## Installazione
Serve solo Python 3.9 o superiore, senza dipendenze.

```bash
git clone https://github.com/juanucilla/travaso && cd travaso
python travaso.py --importa examples/memoria   # oppure la cartella della tua memoria (.md)
python tests/test_travaso.py                   # deve stampare: TUTTI I TEST OK
```

Oppure senza clonare, con uv: `uvx --from git+https://github.com/juanucilla/travaso travaso --stato`

I dati stanno in `~/.travaso`, oppure nella cartella indicata dalla variabile `TRAVASO_DIR`.

## Collegarlo alla tua IA

**Codex / ChatGPT desktop**: in `~/.codex/config.toml`
```toml
[mcp_servers.travaso]
command = "python"
args = ["/percorso/travaso/travaso.py"]
```

**Claude Desktop**: in `claude_desktop_config.json`
```json
{ "mcpServers": { "travaso": { "command": "python", "args": ["/percorso/travaso/travaso.py"] } } }
```

**Claude Code**: `claude mcp add travaso -- python /percorso/travaso/travaso.py`

**Cursor, Gemini CLI, VS Code**: stessa forma `command` + `args` nei rispettivi file di configurazione MCP.

**ChatGPT web (connettori, modalità sviluppatore)**: avvia `python travaso.py --http 8765` ed esponi `http://127.0.0.1:8765/mcp` con un tunnel HTTPS, per esempio `cloudflared tunnel --url http://127.0.0.1:8765`. Poi aggiungi l'URL come connettore.

Nei client che supportano le skill, copia `skill/` nella loro cartella delle skill. Per esempio in Codex: `~/.codex/skills/travaso/`.

## Strumenti
| Tool | Cosa fa |
|---|---|
| `travaso_stato` | Livello del serbatoio (%), fatti rimasti, assorbiti e in archivio |
| `travaso_cerca` | Ricerca che consegna solo fatti nuovi e suggerisce le prossime ricerche |
| `travaso_prossimi` | Consegna i fatti in ordine, per il travaso completo |
| `travaso_assorbi` | Conferma che i fatti sono stati salvati: escono dal serbatoio |
| `travaso_archivio` | Ricerca nell'archivio originale, che non si svuota mai |
| `travaso_ripristina` | Riempie di nuovo il serbatoio dall'archivio |

Con `TRAVASO_AUTO=1` i fatti escono dal serbatoio appena consultati, senza conferma. È comodo, ma se l'IA non li salva davvero si perdono. In ogni caso restano in archivio.

## Formato della memoria
File Markdown, un fatto per riga o per punto elenco, con un frontmatter facoltativo:
```markdown
---
name: leopardi-infinito
aliases: [L'infinito, Leopardi]
description: breve descrizione
---
## Sezione
- un fatto per riga
```

## Esempi inclusi (pubblico dominio)
- `examples/memoria/leopardi-infinito.md`: *L'infinito* di Giacomo Leopardi (1819)
- `examples/memoria/bach-goldberg-aria.md`: le 8 note iniziali del basso dell'Aria delle Variazioni Goldberg (BWV 988), in hertz con LA a 440 e a 432

## Sicurezza
- Travaso gira in locale e non manda dati da nessuna parte.
- All'importazione salta i file che sembrano contenere segreti (password, chiavi, token).

Licenza MIT.

---

### English in brief
Travaso is a zero-dependency MCP server that moves a memory from one AI to another (e.g. Claude → ChatGPT):
- **Preserve:** snapshot to `archivio/`, never modified.
- **Drain:** each query delivers only new facts; the client saves them to its own memory and calls `travaso_assorbi` to remove them.
- **Empty:** it runs until the tank reaches 0%; the archive stays searchable and restorable.

It runs over stdio or streamable HTTP (`--http PORT`).
