---
name: explain
description: Use when the user wants a concept explained from scratch — triggers on "/explain", "explain X to me", "me explica X", "como funciona X", "não entendi X". Also use when the user wants to understand a concept from the current session's work. Delivers a four-part teaching flow in Portuguese (explanation, real-world analogy, mini-code example, then the technical vocabulary for what was shown).
---

# explain

## Overview

Teach one concept in a fixed four-part flow so the user walks away understanding it AND knowing the technical names for it. Language: **Portuguese, but keep technical terms in English** (closure, hoisting, race condition, idempotent...). Never translate jargon — name it in English, explain it in PT.

Core principle: explain the idea before naming it. Names land only after the user already feels the concept.

## When to Use

- User passes a topic → teach that topic. ("/explain closures", "me explica debounce")
- User passes nothing → teach the main concept from the current session's work.
- User says they didn't understand something → use this flow on that thing.

Skip when: user wants a quick factual answer, not understanding. Don't force the four parts on a one-line lookup.

## The Flow (always these 4 parts, in order)

### 1. Explicação
Plain-language explanation of WHAT it is and WHY it exists (what problem it solves). 2–5 sentences. No jargon yet. Build the intuition first.

### 2. Analogia
One real-world analogy that maps to the concept. Make the mapping explicit ("o garçom = a função; o pedido = o argumento"). Analogy must be concrete and everyday.

### 3. Exemplo (analogia → mini-código)
A small, runnable code snippet (< 20 lines) showing the concept in action. Comment the key line(s) explaining WHY. Keep it minimal — one idea, no noise.

**Linguagem: sempre Ruby.** Se o conceito for de web/framework (controllers, ActiveRecord, migrations, rotas, jobs...) → exemplo em **Rails**. Se for conceito básico de programação (loops, blocks, classes, recursão...) → **Ruby puro**, sem Rails.

### 4. Nomes técnicos
Bullet list of the technical terms for what was just shown. Each line: **`termo` (EN)** — short PT gloss. Include only terms that actually appeared in the explanation/example. This is the payoff: the user can now Google and talk about it.

## Output Shape

```
## [Conceito]

**1. O que é**
<explicação simples>

**2. Analogia**
<analogia + mapeamento>

**3. Exemplo**
```<lang>
<mini-código comentado>
```

**4. Nomes técnicos**
- **`term`** — <gloss PT>
- ...
```

## After teaching

Ask ONE check question to confirm it landed (e.g. "Saca por que isso evita X?"). Don't dump a quiz — one question, then stop.

## Common Mistakes

- Naming the jargon before building intuition → user memorizes a word, not a concept. Names go LAST.
- Translating technical terms to Portuguese → user can't search or discuss it. Keep terms in English.
- Example too big → one idea per snippet. Cut everything not load-bearing.
- Abstract analogy → must be everyday and concrete, with explicit mapping.
