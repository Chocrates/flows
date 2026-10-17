/**
 * UI strings. English is the source catalogue; other locales may omit keys
 * and fall back to English.
 */

type Catalogue = Record<string, string>;

const en: Catalogue = {
  "column.add": "Add column",
  "column.rename": "Rename column",
  "column.wipExceeded": "{name} is over its limit of {limit}",
  "card.add": "Add a card",
  "card.count": "{count} cards",
  "card.dueSoon": "Due {when}",
  "card.overdue": "Overdue since {when}",
  "card.archived": "This card is archived",
  "sync.offline": "You're offline. Changes will be sent when you reconnect.",
  "sync.conflict": "Someone else changed this card. Your version was kept as a draft.",
  "sync.failed": "Couldn't save. Retrying…",
  "search.placeholder": "Search titles, labels, people",
  "search.noResults": "Nothing matches “{query}”",
  "reminder.title": "Reminder: {title}",
  "reminder.body": "Due {when}",
  "shortcut.help": "Keyboard shortcuts",
};

const fr: Catalogue = {
  "column.add": "Ajouter une colonne",
  "column.rename": "Renommer la colonne",
  "card.add": "Ajouter une carte",
  "card.count": "{count} cartes",
  "card.dueSoon": "Échéance {when}",
  "sync.offline": "Vous êtes hors ligne. Les modifications seront envoyées à la reconnexion.",
  "search.placeholder": "Rechercher titres, étiquettes, personnes",
};

const CATALOGUES: Record<string, Catalogue> = { en, fr };

export function resolveLocale(requested: string): string {
  if (CATALOGUES[requested]) return requested;
  const base = requested.split("-")[0];
  return CATALOGUES[base] ? base : "en";
}

/**
 * Look up `key` for `locale` and interpolate `{name}` placeholders.
 * Missing placeholders are left verbatim so they are visible in QA.
 */
export function t(locale: string, key: string, params: Record<string, string | number> = {}): string {
  const cat = CATALOGUES[resolveLocale(locale)];
  const template = cat[key] ?? en[key] ?? key;
  return template.replace(/\{(\w+)\}/g, (whole, name: string) =>
    name in params ? String(params[name]) : whole,
  );
}

export function registerCatalogue(locale: string, entries: Catalogue): void {
  CATALOGUES[locale] = { ...(CATALOGUES[locale] ?? {}), ...entries };
}
