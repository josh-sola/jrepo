import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiPost } from '../api.ts';
import type {
  CreateReviewCommentRequest,
  PrResponse,
  ReplyRequest,
  ResolveThreadRequest,
  ThreadsResponse,
} from '../../shared/api.ts';
import type { PrParams } from './usePrData.ts';

function prBasePath({ owner, repo, number }: PrParams): string {
  return `/api/pr/${owner}/${repo}/${number}`;
}

// Every comment write returns the whole thread list, so the mutation writes
// it straight into the PR query's cache instead of waiting on a refetch.
function useApplyThreads(
  params: PrParams,
): (response: ThreadsResponse) => void {
  const queryClient = useQueryClient();
  const queryKey = ['pr', params.owner, params.repo, params.number];
  return (response) => {
    queryClient.setQueryData<PrResponse>(queryKey, (current) =>
      current ? { ...current, threads: response.threads } : current,
    );
  };
}

export function useCreateComment(params: PrParams) {
  const applyThreads = useApplyThreads(params);
  return useMutation({
    mutationFn: (body: CreateReviewCommentRequest) =>
      apiPost<ThreadsResponse>(`${prBasePath(params)}/comments`, body),
    onSuccess: applyThreads,
  });
}

export function useReplyToComment(params: PrParams) {
  const applyThreads = useApplyThreads(params);
  return useMutation({
    mutationFn: ({
      commentId,
      body,
    }: {
      commentId: number;
      body: ReplyRequest;
    }) =>
      apiPost<ThreadsResponse>(
        `${prBasePath(params)}/comments/${commentId}/replies`,
        body,
      ),
    onSuccess: applyThreads,
  });
}

export function useResolveThread(params: PrParams) {
  const applyThreads = useApplyThreads(params);
  return useMutation({
    mutationFn: ({
      threadId,
      body,
    }: {
      threadId: string;
      body: ResolveThreadRequest;
    }) =>
      apiPost<ThreadsResponse>(
        `${prBasePath(params)}/threads/${threadId}/resolve`,
        body,
      ),
    onSuccess: applyThreads,
  });
}
