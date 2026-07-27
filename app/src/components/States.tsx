import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "./Button";

interface StateProps {
  action?: ReactNode;
  detail?: string;
  title: string;
}

interface LoadingStateProps {
  detail?: string;
  title?: string;
}

export function EmptyState({ action, detail, title }: StateProps) {
  return (
    <section className="kosh-state kosh-state--empty">
      <span aria-hidden="true" className="kosh-state__mark">
        ◌
      </span>
      <h2>{title}</h2>
      {detail && <p>{detail}</p>}
      {action}
    </section>
  );
}

export function LoadingState({
  detail = "Working locally…",
  title = "Loading",
}: LoadingStateProps) {
  return (
    <section aria-live="polite" className="kosh-state" role="status">
      <span aria-hidden="true" className="kosh-spinner" />
      <h2>{title}</h2>
      <p>{detail}</p>
    </section>
  );
}

export function ErrorState({ action, detail, title }: StateProps) {
  return (
    <section className="kosh-state kosh-state--error" role="alert">
      <span aria-hidden="true" className="kosh-state__mark">
        !
      </span>
      <h2>{title}</h2>
      {detail && <p>{detail}</p>}
      {action}
    </section>
  );
}

interface ErrorBoundaryProps {
  children: ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Kosh view failed", error, info);
  }

  render() {
    if (this.state.error) {
      return (
        <main className="kosh-fatal">
          <ErrorState
            action={<Button onClick={() => this.setState({ error: null })}>Try again</Button>}
            detail={this.state.error.message}
            title="This view hit a snag"
          />
        </main>
      );
    }
    return this.props.children;
  }
}
