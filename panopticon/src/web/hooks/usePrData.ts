import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiGet, apiPut } from '../api.ts';
import type {
  PrResponse,
  ViewedResponse,
  SetViewedRequest,
} from '../../shared/api.ts';
import type { PrDiff } from '../../shared/diff.ts';
import type { StackResponse } from '../../shared/stack.ts';

export interface PrParams {
  owner: string;
  repo: string;
  number: string;
}

function prBasePath({ owner, repo, number }: PrParams): string {
  return `/api/pr/${owner}/${repo}/${number}`;
}

export function usePr(params: PrParams) {
  return useQuery({
    queryKey: ['pr', params.owner, params.repo, params.number],
    queryFn: () => apiGet<PrResponse>(prBasePath(params)),
  });
}

// Whitespace-only changes are always ignored; there is no toggle. This only
// affects files that fall back to a plain-text diff, since difftastic
// already ignores whitespace for the languages it parses.
export function usePrDiff(params: PrParams) {
  return useQuery({
    queryKey: ['pr-diff', params.owner, params.repo, params.number],
    queryFn: () => apiGet<PrDiff>(`${prBasePath(params)}/diff?ws=ignore`),
  });
}

export function useViewed(params: PrParams) {
  return useQuery({
    queryKey: ['pr-viewed', params.owner, params.repo, params.number],
    queryFn: () => apiGet<ViewedResponse>(`${prBasePath(params)}/viewed`),
  });
}

export function useStack(params: PrParams) {
  return useQuery({
    queryKey: ['pr-stack', params.owner, params.repo, params.number],
    queryFn: () => apiGet<StackResponse>(`${prBasePath(params)}/stack`),
  });
}

export function useSetViewed(params: PrParams) {
  const queryClient = useQueryClient();
  const queryKey = ['pr-viewed', params.owner, params.repo, params.number];
  return useMutation({
    mutationFn: (body: SetViewedRequest) =>
      apiPut<ViewedResponse>(`${prBasePath(params)}/viewed`, body),
    onMutate: async (body) => {
      await queryClient.cancelQueries({ queryKey });
      const previous = queryClient.getQueryData<ViewedResponse>(queryKey);
      queryClient.setQueryData<ViewedResponse>(queryKey, (current) => {
        const viewed = { ...current?.viewed };
        if (body.viewed) viewed[body.path] = body.oid;
        else delete viewed[body.path];
        return { viewed };
      });
      return { previous };
    },
    onError: (_error, _body, context) => {
      if (context?.previous)
        queryClient.setQueryData(queryKey, context.previous);
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey });
    },
  });
}
