import type { ReactNode } from 'react';

export function PageHead({
  title,
  subtitle,
  badge,
  actions,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  badge?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="lc-page-head flex flex-wrap items-start justify-between gap-4">
      <div>
        <h1 className="flex items-center gap-3">
          {title}
          {badge}
        </h1>
        {subtitle && <p>{subtitle}</p>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </header>
  );
}
