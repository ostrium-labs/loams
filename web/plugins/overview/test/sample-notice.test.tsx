import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vite-plus/test';
import { LiveDashboard } from '../src/live/live-dashboard.js';
import { mockOverviewMetrics } from '../src/live/mock.js';

afterEach(cleanup);

it('the live dashboard says its fixture data is a sample, with the shared notice only (UX-5)', async () => {
  const { container } = render(<LiveDashboard client={mockOverviewMetrics()} />);
  await screen.findAllByText('Sample data');
  expect(container.querySelector('.loams-sample-data')).not.toBeNull();
  // The banner says it; a second badge beside the heading repeated it.
  expect(container.querySelector('.loams-sample-data-badge')).toBeNull();
});
