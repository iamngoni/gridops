import { createFileRoute } from "@tanstack/react-router";

import { SettingsLayout, SettingsRow, SettingsSection } from "~/components/settings-ui";
import { useTheme } from "~/components/theme-provider";
import { Shortcut } from "~/components/ui/kbd";
import { allNavItems } from "~/lib/navigation";
import { cn } from "~/lib/utils";

export const Route = createFileRoute("/settings/preferences")({
  component: PreferencesSettings,
});

function PreferencesSettings() {
  const { theme, toggleTheme } = useTheme();
  return (
    <SettingsLayout description="Personal settings for this browser." title="Preferences">
      <SettingsSection title="Appearance">
        <SettingsRow description="GridOps follows Linear’s contrast levels in both themes." label="Theme" stacked>
          <div className="grid grid-cols-2 gap-3">
            {(["light", "dark"] as const).map((option) => (
              <button
                aria-pressed={theme === option}
                className={cn(
                  "group overflow-hidden rounded-lg border text-left transition-colors",
                  theme === option ? "border-primary ring-2 ring-primary/25" : "border-border-strong hover:border-faint",
                )}
                key={option}
                onClick={() => { if (theme !== option) toggleTheme(); }}
                type="button"
              >
                <ThemePreview theme={option} />
                <span className="flex items-center justify-between border-t border-border px-3 py-2 text-sm font-medium capitalize">{option}{theme === option ? <span className="size-2 rounded-full bg-primary" /> : null}</span>
              </button>
            ))}
          </div>
        </SettingsRow>
      </SettingsSection>

      <SettingsSection description="Available anywhere outside a text field." title="Keyboard shortcuts">
        <ShortcutRow keys={["⌘", "K"]} label="Open command menu" />
        <ShortcutRow keys={["C"]} label="Create runner pool" />
        <ShortcutRow alternatives keys={["J", "K"]} label="Move down or up a list" />
        <ShortcutRow keys={["↵"]} label="Open the focused row" />
        {allNavItems.map((item) => <ShortcutRow keys={["G", item.shortcut.toUpperCase()]} key={item.to} label={`Go to ${item.label}`} />)}
      </SettingsSection>
    </SettingsLayout>
  );
}

function ShortcutRow({ label, keys, alternatives }: { label: string; keys: string[]; alternatives?: boolean }) {
  return <div className="flex items-center justify-between px-4 py-2.5 text-sm"><span className="text-secondary-foreground">{label}</span><Shortcut alternatives={alternatives} keys={keys} /></div>;
}

function ThemePreview({ theme }: { theme: "light" | "dark" }) {
  const dark = theme === "dark";
  return (
    <div className={cn("flex h-24 gap-2 p-2.5", dark ? "bg-[#08090a]" : "bg-[#f4f4f6]")}>
      <div className="w-10 space-y-1.5 pt-1">
        {[0, 1, 2, 3].map((line) => <div className={cn("h-1.5 rounded-full", dark ? "bg-white/15" : "bg-black/10", line === 1 && (dark ? "bg-white/35" : "bg-black/25"))} key={line} />)}
      </div>
      <div className={cn("flex-1 space-y-1.5 rounded-md border p-2", dark ? "border-white/10 bg-[#0f1011]" : "border-black/5 bg-white")}>
        {[0, 1, 2].map((line) => (
          <div className="flex items-center gap-1.5" key={line}>
            <span className={cn("size-2 rounded-full", line === 0 ? "bg-[#f2c94c]" : line === 1 ? "bg-[#4cb782]" : "bg-[#5e6ad2]")} />
            <span className={cn("h-1.5 flex-1 rounded-full", dark ? "bg-white/15" : "bg-black/10")} />
          </div>
        ))}
      </div>
    </div>
  );
}
