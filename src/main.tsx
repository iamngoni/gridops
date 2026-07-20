import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RouterProvider } from "@tanstack/react-router";

import { getRouter } from "./router";
import "./styles/app.css";

const container = document.getElementById("root");
if (!container) throw new Error("GridOps root element is missing.");

createRoot(container).render(
  <StrictMode>
    <RouterProvider router={getRouter()} />
  </StrictMode>,
);
