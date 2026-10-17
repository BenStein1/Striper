import React, { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { commands, type CorrectionRecord, type LearnedEntry } from "@/bindings";
import { useSettings } from "../../../hooks/useSettings";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { SettingContainer } from "../../ui/SettingContainer";
import { ToggleSwitch } from "../../ui/ToggleSwitch";
import { Input } from "../../ui/Input";
import { Button } from "../../ui/Button";
import { ShortcutInput } from "../ShortcutInput";
import { PostProcessingSettingsApi } from "../PostProcessingSettingsApi";

const RECENT_CORRECTIONS = 20;

/** Splits a comma-separated field into trimmed, non-empty words. */
const splitList = (value: string) =>
  value
    .split(",")
    .map((part) => part.trim())
    .filter((part) => part.length > 0);

const LearnedEntryRow: React.FC<{
  entry: LearnedEntry;
  onChanged: () => void;
}> = ({ entry, onChanged }) => {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);

  const update = async (patch: { enabled?: boolean; always?: boolean }) => {
    setBusy(true);
    const result = await commands.updateLearnedVocabulary(entry.id, {
      preferred_text: null,
      aliases: null,
      context_words: null,
      negative_context_words: null,
      always: patch.always ?? null,
      app_scope: null,
      enabled: patch.enabled ?? null,
      use_as_asr_hint: null,
    });
    setBusy(false);
    if (result.status === "error") {
      toast.error(result.error);
    }
    onChanged();
  };

  const remove = async () => {
    const confirmed = await ask(
      t("settings.vocabulary.entries.deleteConfirm", {
        word: entry.preferred_text,
      }),
      { kind: "warning" },
    );
    if (!confirmed) return;
    setBusy(true);
    const result = await commands.deleteLearnedVocabulary(entry.id);
    setBusy(false);
    if (result.status === "error") {
      toast.error(result.error);
    }
    onChanged();
  };

  return (
    <div className="px-4 py-3 flex flex-col gap-2">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div
            className={`font-medium ${entry.enabled ? "" : "line-through text-mid-gray"}`}
          >
            {entry.preferred_text}
          </div>
          {entry.aliases.length > 0 && (
            <div className="text-xs text-mid-gray">
              {t("settings.vocabulary.entries.heardAs", {
                words: entry.aliases.join(", "),
              })}
            </div>
          )}
          <div className="text-xs text-mid-gray">
            {entry.always
              ? t("settings.vocabulary.entries.always")
              : entry.context_words.length > 0
                ? t("settings.vocabulary.entries.whenNear", {
                    words: entry.context_words.join(", "),
                  })
                : t("settings.vocabulary.entries.capitalizedOnly")}
          </div>
          {entry.app_scope && (
            <div className="text-xs text-mid-gray">
              {t("settings.vocabulary.entries.onlyIn", {
                app: entry.app_scope,
              })}
            </div>
          )}
        </div>
        <div className="flex flex-wrap gap-2 justify-end">
          <Button
            variant="secondary"
            size="md"
            disabled={busy}
            onClick={() => update({ enabled: !entry.enabled })}
          >
            {entry.enabled
              ? t("settings.vocabulary.entries.disable")
              : t("settings.vocabulary.entries.enable")}
          </Button>
          <Button
            variant="secondary"
            size="md"
            disabled={busy}
            onClick={() => update({ always: !entry.always })}
          >
            {entry.always
              ? t("settings.vocabulary.entries.makeContextual")
              : t("settings.vocabulary.entries.makeAlways")}
          </Button>
          <Button
            variant="danger-ghost"
            size="md"
            disabled={busy}
            onClick={remove}
          >
            {t("settings.vocabulary.entries.delete")}
          </Button>
        </div>
      </div>
    </div>
  );
};

const AddEntryForm: React.FC<{ onAdded: () => void }> = ({ onAdded }) => {
  const { t } = useTranslation();
  const [preferred, setPreferred] = useState("");
  const [aliases, setAliases] = useState("");
  const [context, setContext] = useState("");
  const [always, setAlways] = useState(false);
  const [busy, setBusy] = useState(false);

  const canAdd = preferred.trim().length > 0 && !busy;

  const add = async () => {
    if (!canAdd) return;
    setBusy(true);
    const result = await commands.addLearnedVocabulary(
      preferred.trim(),
      splitList(aliases),
      splitList(context),
      always,
    );
    setBusy(false);
    if (result.status === "error") {
      toast.error(result.error);
      return;
    }
    toast.success(
      t("settings.vocabulary.add.added", { word: result.data.preferred_text }),
    );
    setPreferred("");
    setAliases("");
    setContext("");
    setAlways(false);
    onAdded();
  };

  const onEnter = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      add();
    }
  };

  return (
    <>
      <SettingContainer
        title={t("settings.vocabulary.add.preferred.title")}
        description={t("settings.vocabulary.add.preferred.description")}
        descriptionMode="tooltip"
        grouped={true}
      >
        <Input
          type="text"
          value={preferred}
          onChange={(e) => setPreferred(e.target.value)}
          onKeyDown={onEnter}
          placeholder={t("settings.vocabulary.add.preferred.placeholder")}
          variant="compact"
          disabled={busy}
        />
      </SettingContainer>
      <SettingContainer
        title={t("settings.vocabulary.add.aliases.title")}
        description={t("settings.vocabulary.add.aliases.description")}
        descriptionMode="tooltip"
        grouped={true}
      >
        <Input
          type="text"
          value={aliases}
          onChange={(e) => setAliases(e.target.value)}
          onKeyDown={onEnter}
          placeholder={t("settings.vocabulary.add.aliases.placeholder")}
          variant="compact"
          disabled={busy}
        />
      </SettingContainer>
      <SettingContainer
        title={t("settings.vocabulary.add.context.title")}
        description={t("settings.vocabulary.add.context.description")}
        descriptionMode="tooltip"
        grouped={true}
      >
        <Input
          type="text"
          value={context}
          onChange={(e) => setContext(e.target.value)}
          onKeyDown={onEnter}
          placeholder={t("settings.vocabulary.add.context.placeholder")}
          variant="compact"
          disabled={busy}
        />
      </SettingContainer>
      <ToggleSwitch
        checked={always}
        onChange={setAlways}
        disabled={busy}
        label={t("settings.vocabulary.add.always.label")}
        description={t("settings.vocabulary.add.always.description")}
        descriptionMode="tooltip"
        grouped={true}
      />
      <div className="px-4 py-3 flex justify-end">
        <Button variant="primary" size="md" disabled={!canAdd} onClick={add}>
          {t("settings.vocabulary.add.button")}
        </Button>
      </div>
    </>
  );
};

const RecentCorrections: React.FC<{ records: CorrectionRecord[] }> = ({
  records,
}) => {
  const { t } = useTranslation();
  if (records.length === 0) {
    return (
      <div className="px-4 py-3 text-sm text-mid-gray">
        {t("settings.vocabulary.recent.empty")}
      </div>
    );
  }
  return (
    <>
      {records.map((record) => (
        <div key={record.id} className="px-4 py-3 text-sm flex flex-col gap-1">
          <div className="font-medium">“{record.instruction}”</div>
          {record.before_text !== record.after_text && (
            <div className="text-xs text-mid-gray break-words">
              {t("settings.vocabulary.recent.change", {
                before: record.before_text,
                after: record.after_text,
              })}
            </div>
          )}
          <div className="text-xs text-mid-gray">
            {t("settings.vocabulary.recent.meta", {
              result: record.result,
              interpreter: record.interpreter,
              time: new Date(record.timestamp * 1000).toLocaleString(),
            })}
          </div>
        </div>
      ))}
    </>
  );
};

export const VocabularySettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const [entries, setEntries] = useState<LearnedEntry[]>([]);
  const [records, setRecords] = useState<CorrectionRecord[]>([]);

  const refresh = useCallback(async () => {
    const [vocab, recent] = await Promise.all([
      commands.getLearnedVocabulary(),
      commands.getRecentCorrections(RECENT_CORRECTIONS),
    ]);
    if (vocab.status === "ok") setEntries(vocab.data);
    if (recent.status === "ok") setRecords(recent.data);
  }, []);

  useEffect(() => {
    refresh();
    // Spoken corrections can learn or forget words while this page is open.
    const unlisten = listen("correction-result", () => refresh());
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refresh]);

  const exportVocabulary = async () => {
    const path = await save({
      defaultPath: "striper-vocabulary.json",
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    const result = await commands.exportLearnedVocabulary(path);
    if (result.status === "error") {
      toast.error(result.error);
      return;
    }
    toast.success(
      t("settings.vocabulary.transfer.exported", { count: result.data }),
    );
  };

  const importVocabulary = async () => {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (typeof path !== "string") return;
    const result = await commands.importLearnedVocabulary(path);
    if (result.status === "error") {
      toast.error(result.error);
      return;
    }
    toast.success(
      t("settings.vocabulary.transfer.imported", { count: result.data }),
    );
    refresh();
  };

  const llmEnabled = getSetting("correction_llm_enabled") ?? true;

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup
        title={t("settings.vocabulary.correction.title")}
        description={t("settings.vocabulary.correction.description")}
      >
        <ShortcutInput shortcutId="correct" grouped={true} />
        <ToggleSwitch
          checked={getSetting("learned_vocabulary_enabled") ?? true}
          onChange={(v) => updateSetting("learned_vocabulary_enabled", v)}
          isUpdating={isUpdating("learned_vocabulary_enabled")}
          label={t("settings.vocabulary.correction.applyLearned.label")}
          description={t(
            "settings.vocabulary.correction.applyLearned.description",
          )}
          descriptionMode="tooltip"
          grouped={true}
        />
        <ToggleSwitch
          checked={getSetting("correction_require_readback") ?? true}
          onChange={(v) => updateSetting("correction_require_readback", v)}
          isUpdating={isUpdating("correction_require_readback")}
          label={t("settings.vocabulary.correction.readback.label")}
          description={t("settings.vocabulary.correction.readback.description")}
          descriptionMode="tooltip"
          grouped={true}
        />
        <ToggleSwitch
          checked={llmEnabled}
          onChange={(v) => updateSetting("correction_llm_enabled", v)}
          isUpdating={isUpdating("correction_llm_enabled")}
          label={t("settings.vocabulary.correction.llm.label")}
          description={t("settings.vocabulary.correction.llm.description")}
          descriptionMode="tooltip"
          grouped={true}
        />
      </SettingsGroup>

      {llmEnabled && (
        <SettingsGroup
          title={t("settings.vocabulary.provider.title")}
          description={t("settings.vocabulary.provider.description")}
        >
          <PostProcessingSettingsApi />
        </SettingsGroup>
      )}

      <SettingsGroup
        title={t("settings.vocabulary.add.title")}
        description={t("settings.vocabulary.add.description")}
      >
        <AddEntryForm onAdded={refresh} />
      </SettingsGroup>

      <SettingsGroup
        title={t("settings.vocabulary.entries.title", {
          count: entries.length,
        })}
      >
        {entries.length === 0 ? (
          <div className="px-4 py-3 text-sm text-mid-gray">
            {t("settings.vocabulary.entries.empty")}
          </div>
        ) : (
          entries.map((entry) => (
            <LearnedEntryRow key={entry.id} entry={entry} onChanged={refresh} />
          ))
        )}
        <div className="px-4 py-3 flex gap-2 justify-end">
          <Button variant="secondary" size="md" onClick={importVocabulary}>
            {t("settings.vocabulary.transfer.import")}
          </Button>
          <Button
            variant="secondary"
            size="md"
            disabled={entries.length === 0}
            onClick={exportVocabulary}
          >
            {t("settings.vocabulary.transfer.export")}
          </Button>
        </div>
      </SettingsGroup>

      <SettingsGroup title={t("settings.vocabulary.recent.title")}>
        <RecentCorrections records={records} />
      </SettingsGroup>
    </div>
  );
};
