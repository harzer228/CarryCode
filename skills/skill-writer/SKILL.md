---
name: skill-writer
description: "Create or rewrite a CarryCode skill in the SKILL.md format, including a ready-made ChatGPT prompt for drafting one. Use when the user wants a new skill, asks how to teach the agent a workflow, or wants a skill turned into a reusable instruction file."
---

# Writing a CarryCode skill

A skill is a folder with `SKILL.md`. CarryCode reads the `name` and
`description` from the front matter and shows them in `/skills`; the model loads
the body on demand with `action: skill` and the `name` field. Keep the body as a
procedure, not as documentation.

## Where skills live

| Место | Для чего |
| --- | --- |
| `<project>/.carrycode/skills/<name>/SKILL.md` | только этот проект |
| `<флешка>/skills/<name>/SKILL.md` | вместе с комплектом CarryCode |
| `%USERPROFILE%\.carrycode\skills\<name>\SKILL.md` | все проекты пользователя |
| `%USERPROFILE%\.agents\skills`, `.claude\skills`, `.codex\skills` | читаются как общие, если проекта нет своего |

The first folder that provides a name wins, so a project skill can override a
built-in one. Built-ins (`code-review`, `debug-project`, `skill-writer`) are
compiled into the binary and always available.

## Front matter

```markdown
---
name: my-skill
description: "One or two sentences: when to use this skill and what it produces. Start with a verb; include the phrases a user would say."
---
```

Rules for `description`:

- 1–2 sentences, under 400 characters;
- says **when** to use it, not what the skill contains;
- includes two or three trigger phrases in the user's language;
- no secrets and no file paths that only exist on one machine.

## Body

1. `## When to use` — triggers and non-goals.
2. `## Workflow` — numbered steps, each naming a concrete CarryCode action.
3. `## Output format` — the exact shape of the answer.
4. Limits and safety rules: what the skill must never do.

CarryCode actions available to a skill: `read`, `list`, `search`, `edit`,
`run` (confirmed), `plan`, `skill`, and `mcp`. A skill must not promise
anything outside that list. `run` tasks are limited to `cargo-test`,
`cargo-check`, `npm-test`, `npm-build`, `python-test`.

Keep the body under 6 KiB. Long checklists make the model skim.

## Ready ChatGPT prompt

Copy the block below into ChatGPT and replace the text in `[...]`. It returns a
complete `SKILL.md`; save it to one of the folders above and run `/skills` in
CarryCode.

```
Ты — автор инструкций для консольного агента CarryCode. Помоги мне сделать
переиспользуемый скилл в формате SKILL.md.

Контекст агента:
- агент работает в терминале, видит проект и общается с моделью через JSON-экшены;
- доступные экшены: read (path), list (path), search (query), edit (path,
  reason, before, after), run (task: cargo-test, cargo-check, npm-test,
  npm-build, python-test), plan (text), skill (name), mcp (server, tool,
  arguments);
- edit меняет только существующий файл точной заменой before → after и
  требует подтверждения пользователя;
- run требует подтверждения и выполняется с правами пользователя.

Моя задача: [что должен делать скилл, в каких ситуациях его вызывать].
Проект: [язык, фреймворк, тесты, важные ограничения].
Что должно быть на выходе: [формат, например список находок с severity и
файлом/строкой].
Что скилл делать НЕ должен: [запреты, например не менять код без просьбы].

Верни ровно один файл SKILL.md и ничего больше:
1. YAML front matter между --- с полями name (kebab-case, до 64 символов) и
   description (1–2 предложения, начинается с глагола, содержит фразы-триггеры
   на русском, до 400 символов).
2. Тело на русском: ## Когда использовать, ## Порядок работы (нумерованные
   шаги с конкретными экшенами), ## Формат ответа, ## Ограничения.
3. Тело не больше 6 КиБ, без выдуманных инструментов, без секретов.
4. Если чего-то не хватает для качественного скилла, задай не больше двух
   уточняющих вопросов, а после ответа выдай файл.
```

## Check before saving

- `name` is unique and in kebab-case;
- the description contains the words a user would actually say;
- every step names an action that exists;
- the output format is concrete enough to grade;
- the file is valid UTF-8 without a BOM.

Then open `/skills` in CarryCode: the skill must appear with its description and
source. Ask the agent to use it and confirm that it loads the body instead of
answering from memory.

When asked to save the result, use create_file or apply_changes and show the preview. Reference companion files within the skill folder.
