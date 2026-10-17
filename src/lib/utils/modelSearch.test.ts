import assert from "node:assert/strict";
import i18next from "i18next";
import type { ModelInfo } from "@/bindings";
import catalog from "../../../src-tauri/src/catalog/catalog.json";
import { filterAndSortModels } from "./modelSearch";

const instance = i18next.createInstance();
await instance.init({
  lng: "en",
  resources: {
    en: {
      translation: {
        onboarding: { models: { localized: { name: "Alpha translated" } } },
        customModelDescription: "Custom model",
      },
    },
  },
});
const makeModel = (name: string, id = name): ModelInfo => ({
  id,
  name,
  description: "",
  filename: `${id}.gguf`,
  source: { HuggingFace: { repo_id: "test/models", revision: "main" } },
  size_mb: 1,
  is_downloaded: true,
  is_downloading: false,
  partial_size: 0,
  accuracy_score: 0,
  speed_score: 0,
  supports_translation: false,
  is_recommended: false,
  supported_languages: ["en"],
  supports_language_selection: false,
  is_custom: false,
  supports_streaming: false,
  supports_language_detection: false,
});
const models = catalog.models.map((model) => ({
  ...makeModel(model.name, model.id),
  description: model.description,
}));
const search = (query: string) =>
  filterAndSortModels(models, query, instance.t, "en");
assert.deepEqual(
  search("  TURBO   whisper ").map((model) => model.name),
  ["Whisper Large v3 Turbo"],
);
assert.deepEqual(
  search("parakeet unified").map((model) => model.name),
  ["Parakeet Unified EN 0.6B"],
);
assert.equal(search("this model does not exist").length, 0);
assert.ok(search("handy-computer/parakeet-unified-en").length > 0);
assert.equal(search("ＷＨＩＳＰＥＲ turbo").length, 1);

// Numeric names and visible translations must sort naturally, without mutating the store.
const original = Object.freeze([
  makeModel("Model 10"),
  makeModel("Model 2"),
  makeModel("Zulu", "localized"),
]);
assert.deepEqual(
  filterAndSortModels(original, "", instance.t, "en").map((model) => model.id),
  ["localized", "Model 2", "Model 10"],
);
assert.deepEqual(
  original.map((model) => model.name),
  ["Model 10", "Model 2", "Zulu"],
);
assert.equal(
  filterAndSortModels(original, "alpha", instance.t, "en")[0].id,
  "localized",
);
assert.equal(
  filterAndSortModels(original, "zulu", instance.t, "en")[0].id,
  "localized",
);
assert.equal(filterAndSortModels([], "", instance.t, "en").length, 0);
console.log(
  "Model search: catalog queries, Unicode, translated names, numeric sorting and store immutability passed.",
);
