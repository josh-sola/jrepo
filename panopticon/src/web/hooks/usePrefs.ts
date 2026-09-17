// localStorage is only the optimistic cache for first paint; the server
// stays the source of truth, same as viewed state.
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiGet, apiPut } from '../api.ts';
import type { PrefsResponse, UpsertPrefsRequest } from '../../shared/api.ts';

const STORAGE_KEY = 'panopticon-prefs';
const VIEW_MODE_KEY = 'viewMode';
const WHITESPACE_KEY = 'whitespace';

export type ViewMode = 'split' | 'unified';

function readCache(): Record<string, string> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (parsed && typeof parsed === 'object')
      return parsed as Record<string, string>;
    return {};
  } catch {
    return {};
  }
}

function writeCache(prefs: Record<string, string>): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // best-effort — ignore quota/availability errors
  }
}

export function usePrefsQuery() {
  return useQuery({
    queryKey: ['prefs'],
    queryFn: () => apiGet<PrefsResponse>('/api/prefs'),
    initialData: () => ({ prefs: readCache() }),
  });
}

export function useSetPrefs() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (body: UpsertPrefsRequest) =>
      apiPut<PrefsResponse>('/api/prefs', body),
    onMutate: (body) => {
      queryClient.setQueryData<PrefsResponse>(['prefs'], (current) => {
        const prefs = { ...current?.prefs, ...body };
        writeCache(prefs);
        return { prefs };
      });
    },
  });
}

export function useViewMode(): [ViewMode, (mode: ViewMode) => void] {
  const { data } = usePrefsQuery();
  const { mutate } = useSetPrefs();
  const mode: ViewMode =
    data?.prefs[VIEW_MODE_KEY] === 'unified' ? 'unified' : 'split';
  const setMode = (next: ViewMode) => mutate({ [VIEW_MODE_KEY]: next });
  return [mode, setMode];
}

export function useWhitespaceIgnored(): [boolean, (ignored: boolean) => void] {
  const { data } = usePrefsQuery();
  const { mutate } = useSetPrefs();
  const ignored = data?.prefs[WHITESPACE_KEY] === 'ignore';
  const setIgnored = (next: boolean) => {
    mutate({ [WHITESPACE_KEY]: next ? 'ignore' : 'keep' });
  };
  return [ignored, setIgnored];
}
