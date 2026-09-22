import { Link, useLocation } from 'react-router';
import { cn } from '@/lib/utils';

const VIEWS = [
  { to: '/', label: 'Inbox' },
  { to: '/stacks', label: 'Stacks' },
] as const;

// Segmented switch between the review inbox (/) and the stack view (/stacks),
// shown at the top of both pages.
export function ViewToggle() {
  const location = useLocation();
  return (
    <nav className="inline-flex items-center overflow-hidden rounded-md border border-border">
      {VIEWS.map((view) => {
        const active = location.pathname === view.to;
        return (
          <Link
            key={view.to}
            to={view.to}
            aria-current={active ? 'page' : undefined}
            className={cn(
              'px-3 py-1.5 text-sm font-medium transition-colors',
              active
                ? 'bg-primary text-primary-foreground'
                : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground',
            )}
          >
            {view.label}
          </Link>
        );
      })}
    </nav>
  );
}
