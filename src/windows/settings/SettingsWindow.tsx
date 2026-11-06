import { useState } from "react";
import { EVENTS } from "../../ipc/commands";
import { platform } from "../../platform/platform";
import { useEvent } from "../../state/useResource";
import { AboutPage } from "./AboutPage";
import { ChainPage } from "./ChainPage";
import { ClipboardPage } from "./ClipboardPage";
import { DevicesPage } from "./DevicesPage";
import { searchSections, sections, type SectionId } from "./sections";
import { ShortcutsPage } from "./ShortcutsPage";
import { StartupPage } from "./StartupPage";
import "./settings.css";

const ALL = sections(platform);

function isSection(value: string): value is SectionId {
  return ALL.some((section) => section.id === value);
}

/** The Settings window (#29): a searchable sidebar and one page per section. */
export default function SettingsWindow() {
  const [current, setCurrent] = useState<SectionId>("devices");
  const [query, setQuery] = useState("");
  const [addDevice, setAddDevice] = useState(false);

  // The tray and Rust open a section with `navigate`; `add-device` also opens the sheet.
  useEvent<string>(EVENTS.navigate, (section) => {
    if (section === "add-device") {
      setCurrent("devices");
      setAddDevice(true);
    } else if (isSection(section)) {
      setCurrent(section);
    }
  });

  const results = searchSections(ALL, query);
  const page = (() => {
    switch (current) {
      case "devices":
        return <DevicesPage addDevice={addDevice} onAddDeviceChange={setAddDevice} />;
      case "clipboard":
        return <ClipboardPage />;
      case "chain":
        return <ChainPage />;
      case "shortcuts":
        return <ShortcutsPage />;
      case "startup":
        return <StartupPage />;
      case "about":
        return <AboutPage />;
    }
  })();

  return (
    <div className="settings">
      <nav className="settings__sidebar" aria-label="Settings">
        <input
          className="settings__search"
          type="search"
          placeholder="Search settings"
          aria-label="Search settings"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <ul className="settings__nav">
          {results.map(({ section, matches }) => (
            <li key={section.id}>
              <button
                type="button"
                className={`settings__nav-item${section.id === current ? " settings__nav-item--on" : ""}`}
                aria-current={section.id === current ? "page" : undefined}
                onClick={() => {
                  setCurrent(section.id);
                  setQuery("");
                }}
              >
                {section.title}
              </button>
              {matches.length > 0 && (
                <ul className="settings__matches">
                  {matches.map((match) => (
                    <li key={match}>
                      <button
                        type="button"
                        className="settings__match"
                        onClick={() => {
                          setCurrent(section.id);
                          setQuery("");
                        }}
                      >
                        {match}
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </li>
          ))}
          {results.length === 0 && <li className="settings__empty muted">No settings match.</li>}
        </ul>
      </nav>
      <main className="settings__page">
        <h1 className="settings__title">{ALL.find((section) => section.id === current)?.title}</h1>
        {page}
      </main>
    </div>
  );
}
