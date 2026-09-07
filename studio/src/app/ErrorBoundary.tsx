import { Component, type ErrorInfo, type ReactNode } from "react";
import { log } from "../platform/log";

export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    log("error", error.message, info.componentStack);
  }

  render() {
    if (this.state.error) {
      return (
        <div className="crash">
          <h1>Osvell hit an error</h1>
          <p>{this.state.error.message}</p>
          <button type="button" className="primary" onClick={() => this.setState({ error: null })}>
            Try again
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}
