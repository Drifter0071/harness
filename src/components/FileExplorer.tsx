// @deprecated — use `Sidebar` instead. This file is kept as a re-export
// so any straggler import paths keep resolving. The new sidebar lives
// at `src/components/Sidebar.tsx` and combines Recents + File tree
// into a real layout-flow container (replaces the old floating popover).
export { Sidebar as FileExplorer } from "./Sidebar";
