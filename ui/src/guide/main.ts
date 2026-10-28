import { mount } from "svelte";
import "../lib/theme.css";
import Guide from "./Guide.svelte";

mount(Guide, { target: document.getElementById("app")! });
