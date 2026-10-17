import type { TFunction } from "i18next";
import type { ModelInfo } from "@/bindings";
import {
  getTranslatedModelDescription,
  getTranslatedModelName,
} from "./modelTranslation";

/** Search visible names plus model IDs/descriptions, then sort without mutating the store. */
export function filterAndSortModels(
  models: readonly ModelInfo[],
  query: string,
  t: TFunction,
  language?: string,
): ModelInfo[] {
  const terms = query
    .normalize("NFKC")
    .toLowerCase()
    .trim()
    .split(/\s+/)
    .filter(Boolean);
  const collator = new Intl.Collator(language, {
    numeric: true,
    sensitivity: "base",
  });
  return models
    .map((model) => ({ model, name: getTranslatedModelName(model, t) }))
    .filter(({ model, name }) => {
      const searchable =
        `${name} ${model.name} ${model.id} ${model.description} ${getTranslatedModelDescription(model, t)}`
          .normalize("NFKC")
          .toLowerCase();
      return terms.every((term) => searchable.includes(term));
    })
    .sort(
      (a, b) =>
        collator.compare(a.name, b.name) ||
        collator.compare(a.model.id, b.model.id),
    )
    .map(({ model }) => model);
}
