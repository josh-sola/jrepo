import { useState } from 'react';
import type { FormEvent } from 'react';
import { useNavigate } from 'react-router';
import { Button } from '@/components/ui/button';
import type { OwnerRepo } from '../pages/inboxInput.ts';
import { parsePrInput } from '../pages/inboxInput.ts';

export interface GoToPrFormProps {
  defaultOwnerRepo: OwnerRepo | null;
}

// Shared between the stacks inbox and the review inbox: a quick way to jump
// to a PR by number, owner/repo#number, or a pasted GitHub URL.
export function GoToPrForm({ defaultOwnerRepo }: GoToPrFormProps) {
  const navigate = useNavigate();
  const [input, setInput] = useState('');
  const [inputError, setInputError] = useState<string | null>(null);

  function goToPr(event: FormEvent) {
    event.preventDefault();
    const parsed = parsePrInput(input, defaultOwnerRepo);
    if (!parsed) {
      setInputError(
        defaultOwnerRepo
          ? 'Enter a PR number, owner/repo#number, or a GitHub PR URL.'
          : 'Enter owner/repo#number or a GitHub PR URL — there is no default repo yet.',
      );
      return;
    }
    setInputError(null);
    navigate(`/pr/${parsed.owner}/${parsed.repo}/${parsed.number}`);
  }

  return (
    <div className="flex flex-col gap-2">
      <form onSubmit={goToPr} className="flex gap-2">
        <input
          value={input}
          onChange={(event) => setInput(event.target.value)}
          placeholder="1234, owner/repo#1234, or a GitHub PR URL"
          className="h-9 flex-1 rounded-md border border-input bg-background px-3 text-sm"
        />
        <Button type="submit">Go</Button>
      </form>
      {inputError && <p className="text-sm text-destructive">{inputError}</p>}
    </div>
  );
}
