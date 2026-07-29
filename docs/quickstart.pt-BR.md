# Começando com aag (português do Brasil)

`aag` é um grafo de conhecimento de código que se instala sozinho em todo
agente de programação da máquina. Um binário Rust, sem chave de API, sem passo
de compilação nativa, sem arquivo de configuração.

Tradução do [quickstart em inglês](../README.md). Se as duas divergirem, a
versão em inglês é a correta — é a que é atualizada junto com o código.

## Instalar

```bash
npm install -g @waifucorp/aag

# com busca semântica local (maior, ainda um único arquivo autocontido):
AAG_SEMANTIC=1 npm install -g @waifucorp/aag
```

O postinstall baixa o binário pronto da sua plataforma (Linux, macOS, Windows;
x64 e arm64). Nada compila.

Compilando do código-fonte:

```bash
git clone https://github.com/thewaifucorp/above-all-graphs
cd above-all-graphs && cargo build --release   # binário em target/release/aag
```

## Usar

```bash
aag bigbang   # uma vez por repositório: indexa e conecta todo agente instalado
aag ui        # abre o navegador com todos os seus repositórios
```

`bigbang` faz três coisas de uma vez: indexa o repositório, gera o site offline
em `.aag/`, e registra `aag` em cada agente detectado — servidor MCP, hooks,
skills e regras, cada um no formato que aquele agente usa. É idempotente,
aditivo e reversível: `aag uninstall` remove exatamente o que foi escrito.

## As perguntas que o grafo responde

```bash
aag explore "como o parser resolve imports"   # como algo funciona, com o código junto
aag impact Graph                              # o que quebra se isso mudar
aag rename antigo novo --write                # rename coordenado entre arquivos
git diff --name-only | aag affected --stdin   # quais testes essa mudança atinge
aag areas                                     # que áreas o repositório tem
aag graph-diff main workspace                 # o que este branch fez no grafo
```

Toda aresta carrega a confiança com que foi resolvida — `EXTRACTED` (explícito
no código), `INFERRED` (heurística) ou `AMBIGUOUS` (não resolvido com certeza).
Confira as `AMBIGUOUS` antes de confiar nelas.

## Dentro do agente

Depois do `bigbang`, seu agente já tem a ferramenta MCP `explore` listada e as
skills instaladas. Não precisa configurar nada: pergunte em linguagem natural
("como o login funciona?", "o que quebra se eu mudar `Store`?") e o agente
consulta o grafo em vez de sair fazendo grep.

O índice se mantém fresco sozinho — watcher nativo, reconciliação a cada
conexão MCP, e hooks que ressincronizam depois de cada edição. Não existe
comando de "reindexar" para lembrar.

## Onde ler mais

- [Arquitetura](architecture.md) — como o pipeline funciona
- [Matriz de compatibilidade](compatibility.md) — linguagens, agentes, plataformas
- [Benchmarks](benchmarks.md) — números medidos, com os limites conhecidos
- [Notas de migração](migration.md) — o que muda entre versões
