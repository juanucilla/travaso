# Prompt per le infografiche con Google Stitch

Da usare su stitch.withgoogle.com oppure con l'MCP stitch in Claude Code. Stile comune da incollare in ogni prompt: *palette vino (#1a0f14 fondo, #9e1b3b bordeaux, #e6b450 oro), titoli Fraunces, testo Inter, formato infografica verticale 1080×1350, niente loghi di marchi.*

1. **Hero**: due botti di legno affiancate, collegate da un arco di vino. La botte a sinistra si chiama "Claude · 23%" ed è quasi vuota, quella a destra "ChatGPT · 77%" è quasi piena. Sotto, un cartiglio: "archivio 100% intatto".
2. **Tre gesti**: tre riquadri numerati, "1 Conserva" (scrigno con lucchetto), "2 Scarica" (goccia che passa da una botte all'altra), "3 Svuota" (botte vuota con un indicatore a 0%).
3. **Pirata medievale**: pergamena con un galeone, e la poesia della scheda "Pirata medievale" di index.html.
4. **Napoletano**: il Vesuvio al tramonto, con la poesia napoletana.
5. **Calabrese**: un peperoncino e il mare, con la poesia calabrese.
6. **Thai**: un tempio stilizzato, con la poesia thai (font Noto Sans Thai).
7. **Navajo**: il deserto con le mesas, con il canto in diné bizaad e la nota sull'aspirazione.

Quando le immagini sono pronte, salvale come `site/img/1-hero.png` … `site/img/7-navajo.png` e sostituisci i relativi `<svg>` in index.html con `<img src="img/…" alt="…">`.
