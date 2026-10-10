import type { ReactNode } from 'react';

/** The page header every desktop page uses: optional crumbs, title (+badge), subtitle, actions. */
export function PageHead({
  title,
  subtitle,
  badge,
  actions,
  crumbs,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  badge?: ReactNode;
  actions?: ReactNode;
  crumbs?: ReactNode;
}) {
  return (
    <header
      className={
        crumbs
          ? 'lc-page-head flex flex-wrap items-end justify-between gap-4'
          : 'lc-page-head flex flex-wrap items-start justify-between gap-4'
      }
    >
      <div>
        {crumbs && <nav className="lc-crumbs mb-1 text-sm text-muted">{crumbs}</nav>}
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
