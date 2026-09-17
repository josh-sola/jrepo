import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createBrowserRouter, RouterProvider } from 'react-router';
import { InboxPage, ReviewPage } from './App.tsx';
import { ThemeProvider } from './theme.tsx';
import './index.css';

const router = createBrowserRouter([
  { path: '/', Component: InboxPage },
  { path: '/pr/:owner/:repo/:number', Component: ReviewPage },
]);

const queryClient = new QueryClient();

const rootElement = document.getElementById('root');
if (!rootElement) throw new Error('missing #root element');

createRoot(rootElement).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <ThemeProvider>
        <RouterProvider router={router} />
      </ThemeProvider>
    </QueryClientProvider>
  </StrictMode>,
);
