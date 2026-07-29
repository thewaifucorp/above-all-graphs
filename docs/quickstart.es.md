# Primeros pasos con aag (español)

`aag` es un grafo de conocimiento del código que se instala solo en cada agente
de programación de la máquina. Un binario de Rust: sin clave de API, sin paso
de compilación nativa, sin archivo de configuración.

Traducción del [quickstart en inglés](../README.md). Si ambas divergen, la
versión en inglés es la correcta — es la que se actualiza junto al código.

## Instalación

```bash
npm install -g @waifucorp/aag

# con búsqueda semántica local (más grande, sigue siendo un único archivo):
AAG_SEMANTIC=1 npm install -g @waifucorp/aag
```

El postinstall descarga el binario ya compilado para tu plataforma (Linux,
macOS, Windows; x64 y arm64). No se compila nada.

Desde el código fuente:

```bash
git clone https://github.com/thewaifucorp/above-all-graphs
cd above-all-graphs && cargo build --release   # binario en target/release/aag
```

## Uso

```bash
aag bigbang   # una vez por repositorio: lo indexa y conecta cada agente instalado
aag ui        # abre el navegador con todos tus repositorios
```

`bigbang` hace tres cosas de una vez: indexa el repositorio, genera el sitio
offline en `.aag/`, y registra `aag` en cada agente detectado — servidor MCP,
hooks, skills y reglas, cada uno en el formato que ese agente usa. Es
idempotente, aditivo y reversible: `aag uninstall` elimina exactamente lo que
se escribió.

## Las preguntas que responde el grafo

```bash
aag explore "cómo el parser resuelve los imports"   # cómo funciona algo, con el código
aag impact Graph                                    # qué se rompe si esto cambia
aag rename viejo nuevo --write                      # renombrado coordinado entre archivos
git diff --name-only | aag affected --stdin         # qué pruebas toca este cambio
aag areas                                           # qué áreas tiene el repositorio
aag graph-diff main workspace                       # qué hizo esta rama en el grafo
```

Cada arista lleva la confianza con la que se resolvió: `EXTRACTED` (explícito
en el código), `INFERRED` (heurística) o `AMBIGUOUS` (sin resolver con
certeza). Verifica las `AMBIGUOUS` antes de confiar en ellas.

## Dentro del agente

Después de `bigbang`, tu agente ya tiene la herramienta MCP `explore` listada y
las skills instaladas. No hay nada que configurar: pregunta en lenguaje natural
("¿cómo funciona el login?", "¿qué se rompe si cambio `Store`?") y el agente
consulta el grafo en vez de ponerse a hacer grep.

El índice se mantiene fresco solo — watcher nativo, reconciliación en cada
conexión MCP, y hooks que resincronizan tras cada edición. No hay ningún
comando de "reindexar" que recordar.

## Para seguir leyendo

- [Arquitectura](architecture.md) — cómo funciona el pipeline
- [Matriz de compatibilidad](compatibility.md) — lenguajes, agentes, plataformas
- [Benchmarks](benchmarks.md) — números medidos, con sus límites conocidos
- [Notas de migración](migration.md) — qué cambia entre versiones
