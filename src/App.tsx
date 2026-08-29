/**
 * Astra OS — Root application component
 *
 * Phase 1 renders the desktop shell and its managed applications.
 */
import { Desktop } from "./desktop";
import { AuthGate } from "./security";
import { BootScreen } from "./lifecycle";

export default function App() {
  return (
    <BootScreen>
      {(boot) => (
        <AuthGate>
          {(session) => (
            <Desktop
              authenticated={session.authenticated}
              onAuthenticated={session.onAuthenticated}
              onLogout={session.logout}
              resumeSession={boot.resume_session}
            />
          )}
        </AuthGate>
      )}
    </BootScreen>
  );
}
