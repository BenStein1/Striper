import React, { useEffect, useMemo, useRef, useState } from "react";
import { filterAndSortModels } from "@/lib/utils/modelSearch";
import { ModelComputeBadge, ModelComputeSummary } from "./ModelComputeInfo";
import { useTranslation } from "react-i18next";
import type { ModelInfo } from "@/bindings";
import {
  getTranslatedModelName,
  getTranslatedModelDescription,
} from "../../lib/utils/modelTranslation";

interface ModelDropdownProps {
  models: ModelInfo[];
  currentModelId: string;
  onModelSelect: (modelId: string) => void;
  onClose: () => void;
}

const ModelDropdown: React.FC<ModelDropdownProps> = ({
  models,
  currentModelId,
  onModelSelect,
  onClose,
}) => {
  const { t, i18n } = useTranslation();
  const [query, setQuery] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);
  const resultsRef = useRef<HTMLDivElement>(null);
  const downloadedModels = useMemo(
    () => models.filter((model) => model.is_downloaded),
    [models],
  );
  const visibleModels = useMemo(
    () =>
      filterAndSortModels(downloadedModels, query, t, i18n.resolvedLanguage),
    [downloadedModels, query, t, i18n.resolvedLanguage],
  );
  useEffect(() => {
    searchRef.current?.focus();
  }, []);

  const handleModelClick = (modelId: string) => {
    onModelSelect(modelId);
  };

  return (
    <div
      role="dialog"
      aria-label={t("settings.models.title")}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          onClose();
        }
      }}
      className="absolute bottom-full start-0 mb-2 w-80 max-w-[calc(100vw-2rem)] max-h-[60vh] flex flex-col bg-background border border-mid-gray/20 rounded-lg shadow-lg z-50"
    >
      <div className="p-3 space-y-2 border-b border-mid-gray/20 shrink-0">
        <input
          ref={searchRef}
          type="search"
          aria-label={t("settings.models.searchPlaceholder")}
          placeholder={t("settings.models.searchPlaceholder")}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown") {
              event.preventDefault();
              resultsRef.current
                ?.querySelector<HTMLButtonElement>("button")
                ?.focus();
            }
          }}
          className="w-full px-3 py-2 text-sm bg-mid-gray/10 border border-mid-gray/40 rounded-lg focus:outline-none focus:ring-1 focus:ring-logo-primary"
        />
        <ModelComputeSummary />
      </div>
      <div ref={resultsRef} className="min-h-0 overflow-y-auto py-2">
        {visibleModels.length > 0 ? (
          <div>
            {visibleModels.map((model) => (
              <button
                type="button"
                key={model.id}
                onClick={() => handleModelClick(model.id)}
                aria-pressed={currentModelId === model.id}
                className={`w-full px-3 py-2 text-start hover:bg-mid-gray/10 transition-colors cursor-pointer focus:outline-none focus-visible:bg-mid-gray/10 focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-logo-primary ${
                  currentModelId === model.id
                    ? "bg-logo-primary/10 text-logo-primary"
                    : ""
                }`}
              >
                <div className="flex items-center justify-between">
                  <div>
                    <div className="text-sm text-text/80">
                      {getTranslatedModelName(model, t)}
                      {model.is_custom && (
                        <span className="ms-1.5 text-[10px] font-medium text-text/40 uppercase">
                          {t("modelSelector.custom")}
                        </span>
                      )}
                      {model.supports_streaming && (
                        <span className="ms-1.5 text-[10px] font-medium text-logo-primary/70 uppercase">
                          {t("modelSelector.streaming")}
                        </span>
                      )}
                    </div>
                    <ModelComputeBadge />
                    <div className="text-xs text-text/40 italic pe-4">
                      {getTranslatedModelDescription(model, t)}
                    </div>
                  </div>
                  {currentModelId === model.id && (
                    <div className="text-xs text-logo-primary">
                      {t("modelSelector.active")}
                    </div>
                  )}
                </div>
              </button>
            ))}
          </div>
        ) : (
          <div className="px-3 py-2 text-sm text-text/60">
            {t(
              downloadedModels.length > 0
                ? "settings.models.noModelsMatch"
                : "modelSelector.noModelsAvailable",
            )}
          </div>
        )}
      </div>
    </div>
  );
};

export default ModelDropdown;
