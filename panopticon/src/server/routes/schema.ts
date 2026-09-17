export const PREFS_SCHEMA = `
CREATE TABLE IF NOT EXISTS prefs (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
`;
