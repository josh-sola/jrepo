import { useEffect, useRef, useState, type KeyboardEvent } from 'react';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';

export interface CommentComposerProps {
  onSubmit: (body: string) => void;
  onCancel: () => void;
  submitting: boolean;
  error: string | null;
  submitLabel?: string;
  placeholder?: string;
  headerText?: string;
  focusOnMount?: boolean;
}

export function CommentComposer({
  onSubmit,
  onCancel,
  submitting,
  error,
  submitLabel = 'Comment',
  placeholder = 'Leave a comment',
  headerText,
  focusOnMount,
}: CommentComposerProps) {
  const [body, setBody] = useState('');
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (focusOnMount) textareaRef.current?.focus();
  }, [focusOnMount]);

  function submit(): void {
    const trimmed = body.trim();
    if (!trimmed || submitting) return;
    onSubmit(trimmed);
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (event.key === 'Escape') {
      event.stopPropagation();
      onCancel();
      return;
    }
    if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      submit();
    }
  }

  return (
    <div className="flex w-full max-w-xl flex-col gap-2 rounded-md border border-border bg-card p-2 shadow-sm">
      {headerText && (
        <div className="text-xs font-medium text-muted-foreground">
          {headerText}
        </div>
      )}
      <Textarea
        ref={textareaRef}
        value={body}
        onChange={(event) => setBody(event.target.value)}
        onKeyDown={onKeyDown}
        placeholder={placeholder}
        disabled={submitting}
        className="text-xs"
      />
      {error && (
        <div role="alert" className="text-xs text-destructive">
          {error}
        </div>
      )}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" size="sm" onClick={onCancel}>
          Cancel
        </Button>
        <Button
          type="button"
          size="sm"
          disabled={!body.trim() || submitting}
          onClick={submit}
        >
          {submitting ? 'Posting…' : submitLabel}
        </Button>
      </div>
    </div>
  );
}
