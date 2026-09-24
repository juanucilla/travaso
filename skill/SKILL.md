---
name: travaso
description: Travasa nella tua memoria permanente la memoria ereditata da un'altra IA, servita dall'MCP "travaso", consultandola fino a svuotarla. Usala quando l'utente chiede di importare, trasferire o travasare una memoria, o quando l'MCP travaso ha ancora fatti nel serbatoio.
---

# Travaso: assorbire una memoria fino a svuotarla

L'MCP `travaso` contiene un **serbatoio** di fatti sull'utente, venuti da un'altra IA, e un **archivio** che non cambia mai. Il tuo compito è far passare i fatti nella tua memoria permanente fino a svuotare il serbatoio.

## Procedura
1. Chiama `travaso_stato`. Se `vuoto` è true il travaso è già finito: per ricordare qualcosa usa `travaso_archivio`.
2. **Travaso mirato**, quando c'è una richiesta concreta: usa `travaso_cerca` con i termini della richiesta, poi rilancia con i `next_queries` pertinenti finché `exhausted_for_query` è true.
3. **Travaso completo**, quando l'utente vuole trasferire tutto: usa `travaso_prossimi` finché `exhausted` è true.
4. Dopo **ogni** blocco di risultati:
   - salva nella tua memoria permanente i fatti durevoli: in ChatGPT la memoria ("ricorda che…"); in Codex, Claude o altri il loro meccanismo di memoria;
   - chiama `travaso_assorbi` con gli `id` di `da_assorbire` e con `dove` (per esempio "memoria ChatGPT"). Solo così i fatti escono dal serbatoio.
   - Se un fatto non merita di essere salvato (è vecchio o irrilevante), assorbilo comunque, così il serbatoio si svuota, e dillo all'utente.
5. Alla fine riferisci in una riga quanti fatti hai travasato e che il serbatoio è vuoto (`serbatoio_percento: 0`).

## Regole
- Non salvare mai password, token, chiavi o numeri di carta. Travaso rifiuta all'importazione i file che li contengono.
- Non assorbire fatti che non hai davvero salvato: se il salvataggio fallisce, lasciali nel serbatoio.
- `travaso_ripristina` solo se l'utente lo chiede esplicitamente.
