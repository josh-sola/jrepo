import { useQuery } from '@tanstack/react-query';
import { apiGet } from '../api.ts';
import type { ReviewInboxResponse } from '../../shared/reviewInbox.ts';

const REFETCH_MS = 60_000;

export function useReviewInbox() {
  return useQuery({
    queryKey: ['review-inbox'],
    queryFn: () => apiGet<ReviewInboxResponse>('/api/review-inbox'),
    refetchInterval: REFETCH_MS,
  });
}
