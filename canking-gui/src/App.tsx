import { CanKingDataProvider } from "@kvaser/canking-api/controls";
import { useResizeDetector } from "react-resize-detector";
import { lazy, Suspense } from "react";

const WorkspaceView = lazy(() => import("./WorkspaceView"));

function App() {
  // Get this view's unique id from search params
  const idString = new URLSearchParams(window.location.search).get("id");
  const id = idString !== null ? Number.parseInt(idString, 10) : -1;

  // Use the useResizeDetector hook to make the view responsive to size changes
  const { ref, height, width } = useResizeDetector();

  // The CanKingDataProvider is required to use any of the CanKing API hooks or components
  return (
    <CanKingDataProvider>
      <div ref={ref} style={{ height: "100%", width: "100%" }}>
        <Suspense fallback={null}>
          {id !== -1 && !Number.isNaN(id) && height && width && (
            <WorkspaceView id={id} height={height} width={width} />
          )}
        </Suspense>
      </div>
    </CanKingDataProvider>
  );
}

export default App;
