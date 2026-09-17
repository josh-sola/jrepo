import { useQuery } from '@tanstack/react-query';
import { apiGet } from '../api.ts';
import type { InboxResponse } from '../../shared/inbox.ts';

const REFETCH_MS = 60_000;

export function useInbox() {
  return useQuery({
    queryKey: ['inbox'],
    queryFn: () => apiGet<InboxResponse>('/api/inbox'),
    refetchInterval: REFETCH_MS,
  });
}
