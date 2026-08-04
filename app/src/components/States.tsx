import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "./Button";
import { KoshText } from "./KoshText";
import { KoshTextTone, KoshTextVariant } from "./kosh-text-types";

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
      <KoshText as="h2" variant={KoshTextVariant.Subheading}>
        {title}
      </KoshText>
      {detail && (
        <KoshText as="p" tone={KoshTextTone.Muted} variant={KoshTextVariant.Supporting}>
          {detail}
        </KoshText>
      )}
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
      <KoshText as="h2" variant={KoshTextVariant.Subheading}>
        {title}
      </KoshText>
      <KoshText as="p" tone={KoshTextTone.Muted} variant={KoshTextVariant.Supporting}>
        {detail}
      </KoshText>
    </section>
  );
}

export function ErrorState({ action, detail, title }: StateProps) {
  return (
    <section className="kosh-state kosh-state--error" role="alert">
      <span aria-hidden="true" className="kosh-state__mark">
        !
      </span>
      <KoshText as="h2" variant={KoshTextVariant.Subheading}>
        {title}
      </KoshText>
      {detail && (
        <KoshText as="p" tone={KoshTextTone.Danger} variant={KoshTextVariant.Supporting}>
          {detail}
        </KoshText>
      )}
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
