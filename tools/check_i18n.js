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
