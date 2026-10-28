import { mount } from "svelte";
import "../lib/theme.css";
import Overlay from "./Overlay.svelte";

mount(Overlay, { target: document.getElementById("app")! });
