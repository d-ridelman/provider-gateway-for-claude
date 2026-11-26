// Checks the UI dictionary: same keys in ru and en, no Cyrillic in en, every key the
// markup and app.js use exists, and no stray Cyrillic text outside the dictionary.
// Usage: node tools/check_i18n.js
"use strict";
const fs = require("fs");
const path = require("path");

const ui = path.join(__dirname, "..", "crates", "ctp-app", "ui");
const read = (name) => fs.readFileSync(path.join(ui, name), "utf8");
const STRINGS = new Function(`${read("i18n.js")}; return STRINGS;`)();
const cyrillic = /[Ѐ-ӿ]/;
const problems = [];

const ru = Object.keys(STRINGS.ru);
const en = Object.keys(STRINGS.en);
for (const key of ru) if (!(key in STRINGS.en)) problems.push(`only in ru: ${key}`);
for (const key of en) if (!(key in STRINGS.ru)) problems.push(`only in en: ${key}`);
for (const key of en) if (cyrillic.test(STRINGS.en[key])) problems.push(`Cyrillic in en: ${key}`);

const markup = read("index.html");
const app = read("app.js");
const used = new Set(
  [...`${markup}\n${app}`.matchAll(/(?:data-i18n(?:-html|-title|-placeholder|-aria)?="|\bt\(")([\w.]+)/g)].map((m) => m[1]),
);
// Every string literal inside a t(...) call, so keys picked by a ternary or a template count too.
for (let at = app.indexOf("t("); at >= 0; at = app.indexOf("t(", at + 2)) {
  if (/[\w$.]/.test(app[at - 1] ?? "")) continue;
  let depth = 0;
  let end = at + 1;
  for (; end < app.length; end++) {
    if (app[end] === "(") depth++;
    else if (app[end] === ")" && --depth === 0) break;
  }
  for (const lit of app.slice(at + 2, end).matchAll(/["']([A-Za-z][\w.]*)["']/g)) {
    if (/^[a-z][\w]*(?:\.[\w]+)+$/.test(lit[1])) used.add(lit[1]);
  }
}
// data-help="topic" opens help.<topic>.title / .body.
for (const m of markup.matchAll(/data-help="(\w+)"/g)) used.add(`help.${m[1]}.title`).add(`help.${m[1]}.body`);
for (const key of used) if (!(key in STRINGS.en)) problems.push(`used but missing: ${key}`);

// Cyrillic outside the dictionary: only preset labels ({ ru: … }) and the language name.
const stray = (text, file) =>
  text.split("\n").forEach((line, i) => {
    if (cyrillic.test(line) && !/ru: "/.test(line) && !/>Русский</.test(line)) {
      problems.push(`${file}:${i + 1}: ${line.trim().slice(0, 80)}`);
    }
  });
stray(markup, "index.html");
stray(app, "app.js");

console.log(`keys: ru ${ru.length}, en ${en.length}, used ${used.size}`);
if (problems.length) {
  console.log(problems.join("\n"));
  process.exit(1);
}
console.log("i18n ok");
