import { Routes, Route } from "react-router-dom";
import Home from "./pages/Home";
import GettingStarted from "./pages/GettingStarted";
import Architecture from "./pages/Architecture";
import Commands from "./pages/Commands";
import Branching from "./pages/Branching";
import Environment from "./pages/Environment";
import CrateCore from "./pages/crates/CrateCore";
import CrateState from "./pages/crates/CrateState";
import CrateNix from "./pages/crates/CrateNix";
import CrateMerge from "./pages/crates/CrateMerge";

export default function App() {
  return (
    <Routes>
      <Route path="/" element={<Home />} />
      <Route path="/getting-started" element={<GettingStarted />} />
      <Route path="/architecture" element={<Architecture />} />
      <Route path="/guides/commands" element={<Commands />} />
      <Route path="/guides/branching" element={<Branching />} />
      <Route path="/guides/environment" element={<Environment />} />
      <Route path="/crates/core" element={<CrateCore />} />
      <Route path="/crates/state" element={<CrateState />} />
      <Route path="/crates/nix" element={<CrateNix />} />
      <Route path="/crates/merge" element={<CrateMerge />} />
    </Routes>
  );
}
