const PREF_KEY = "linkvault.newspaper.preferences";

type NewspaperStoredPreferences = {
  destination?: string;
};

function readStoredPreferences(): NewspaperStoredPreferences {
  try {
    return JSON.parse(window.localStorage.getItem(PREF_KEY) ?? "{}") as NewspaperStoredPreferences;
  } catch {
    return {};
  }
}

export function readNewspaperDestination(): string {
  return readStoredPreferences().destination ?? "";
}

export function writeNewspaperDestination(destination: string): void {
  const stored = readStoredPreferences();
  window.localStorage.setItem(
    PREF_KEY,
    JSON.stringify({
      ...stored,
      destination
    })
  );
}
