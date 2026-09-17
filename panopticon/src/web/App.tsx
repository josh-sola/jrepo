import { useParams } from 'react-router';

export function InboxPage() {
  return (
    <div className="p-6">
      <h1 className="text-2xl font-semibold">Inbox</h1>
      <p className="text-muted-foreground">
        Open pull requests will list here.
      </p>
    </div>
  );
}

export function ReviewPage() {
  const { owner, repo, number } = useParams();
  return (
    <div className="p-6">
      <h1 className="text-2xl font-semibold">Review</h1>
      <p className="text-muted-foreground">
        {owner}/{repo}#{number}
      </p>
    </div>
  );
}
