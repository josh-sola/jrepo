import { describe, expect, test } from 'bun:test';
import type { ReviewInboxCandidate, ReviewInboxReview } from '../github/api.ts';
import { categorizeReviewInbox } from './categorize.ts';

function fakeReview(
  overrides: Partial<ReviewInboxReview> = {},
): ReviewInboxReview {
  return {
    state: 'APPROVED',
    submittedAt: '2026-01-01T00:00:00Z',
    authorLogin: 'reviewer',
    authorAvatarUrl: 'https://avatars.githubusercontent.com/u/9?v=4',
    isHuman: true,
    ...overrides,
  };
}

function fakeCandidate(
  overrides: Partial<ReviewInboxCandidate> = {},
): ReviewInboxCandidate {
  return {
    owner: 'acme',
    repo: 'widgets',
    number: 1,
    title: 'A pull request',
    url: 'https://github.com/acme/widgets/pull/1',
    draft: false,
    createdAt: '2026-01-01T00:00:00Z',
    updatedAt: '2026-01-01T00:00:00Z',
    additions: 1,
    deletions: 1,
    author: { login: 'josh', avatarUrl: null, isBot: false },
    reviews: [],
    ...overrides,
  };
}

describe('categorizeReviewInbox', () => {
  test('puts a non-draft PR with a human CHANGES_REQUESTED in returned', () => {
    const pr = fakeCandidate({
      number: 1,
      reviews: [
        fakeReview({
          state: 'CHANGES_REQUESTED',
          authorLogin: 'ann',
          authorAvatarUrl: 'https://avatars.githubusercontent.com/u/4?v=4',
        }),
      ],
    });

    const sections = categorizeReviewInbox('josh', [pr], []);

    expect(sections.returned.map((i) => i.number)).toEqual([1]);
    expect(sections.approved).toEqual([]);
    expect(sections.waiting).toEqual([]);
    expect(sections.drafts).toEqual([]);
    expect(sections.returned[0]?.reviewers).toEqual([
      {
        login: 'ann',
        avatarUrl: 'https://avatars.githubusercontent.com/u/4?v=4',
        state: 'CHANGES_REQUESTED',
      },
    ]);
  });

  test('falls back to an empty avatarUrl when the review author has none', () => {
    const pr = fakeCandidate({
      number: 12,
      reviews: [
        fakeReview({
          state: 'APPROVED',
          authorLogin: 'ann',
          authorAvatarUrl: null,
        }),
      ],
    });

    const sections = categorizeReviewInbox('josh', [pr], []);

    expect(sections.approved[0]?.reviewers).toEqual([
      { login: 'ann', avatarUrl: '', state: 'APPROVED' },
    ]);
  });

  test('puts a non-draft PR with a human APPROVED and no CHANGES_REQUESTED in approved', () => {
    const pr = fakeCandidate({
      number: 2,
      reviews: [fakeReview({ state: 'APPROVED', authorLogin: 'ann' })],
    });

    const sections = categorizeReviewInbox('josh', [pr], []);

    expect(sections.approved.map((i) => i.number)).toEqual([2]);
    expect(sections.returned).toEqual([]);
    expect(sections.waiting).toEqual([]);
  });

  test('puts a non-draft PR with no opinionated review in waiting', () => {
    const pr = fakeCandidate({ number: 3, reviews: [] });

    const sections = categorizeReviewInbox('josh', [pr], []);

    expect(sections.waiting.map((i) => i.number)).toEqual([3]);
  });

  test('a draft always lands in drafts, even with a CHANGES_REQUESTED review', () => {
    const pr = fakeCandidate({
      number: 4,
      draft: true,
      reviews: [fakeReview({ state: 'CHANGES_REQUESTED' })],
    });

    const sections = categorizeReviewInbox('josh', [pr], []);

    expect(sections.drafts.map((i) => i.number)).toEqual([4]);
    expect(sections.returned).toEqual([]);
  });

  test('a CHANGES_REQUESTED review takes precedence over an APPROVED one', () => {
    const pr = fakeCandidate({
      number: 5,
      reviews: [
        fakeReview({ state: 'APPROVED', authorLogin: 'ann' }),
        fakeReview({ state: 'CHANGES_REQUESTED', authorLogin: 'bo' }),
      ],
    });

    const sections = categorizeReviewInbox('josh', [pr], []);

    expect(sections.returned.map((i) => i.number)).toEqual([5]);
    expect(sections.approved).toEqual([]);
  });

  test('ignores a bot review when deciding a PR authored by the login', () => {
    const pr = fakeCandidate({
      number: 6,
      reviews: [
        fakeReview({
          state: 'CHANGES_REQUESTED',
          authorLogin: 'dependabot[bot]',
          isHuman: false,
        }),
      ],
    });

    const sections = categorizeReviewInbox('josh', [pr], []);

    expect(sections.waiting.map((i) => i.number)).toEqual([6]);
    expect(sections.returned).toEqual([]);
  });

  test('puts a requested-review PR authored by someone else in needsReview', () => {
    const pr = fakeCandidate({
      number: 7,
      author: { login: 'ann', avatarUrl: null, isBot: false },
    });

    const sections = categorizeReviewInbox('josh', [], [pr]);

    expect(sections.needsReview.map((i) => i.number)).toEqual([7]);
  });

  test('dedupes a PR that appears in both the authored and requested lists', () => {
    const pr = fakeCandidate({ number: 8 });

    const sections = categorizeReviewInbox('josh', [pr], [pr]);

    expect(sections.needsReview).toEqual([]);
    expect(sections.waiting.map((i) => i.number)).toEqual([8]);
  });

  test('excludes a PR authored by the login itself from needsReview', () => {
    const pr = fakeCandidate({
      number: 9,
      author: { login: 'josh', avatarUrl: null, isBot: false },
    });

    const sections = categorizeReviewInbox('josh', [], [pr]);

    expect(sections.needsReview).toEqual([]);
  });

  test('sorts each section by updatedAt descending', () => {
    const older = fakeCandidate({
      number: 10,
      updatedAt: '2026-01-01T00:00:00Z',
    });
    const newer = fakeCandidate({
      number: 11,
      updatedAt: '2026-02-01T00:00:00Z',
    });

    const sections = categorizeReviewInbox('josh', [older, newer], []);

    expect(sections.waiting.map((i) => i.number)).toEqual([11, 10]);
  });
});
