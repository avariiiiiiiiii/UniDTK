/**
 * UniDTK renderer — entry point.
 *
 * All application logic lives in the core module (core/ui/unidtk-core.js) and in
 * domain modules (modules/<id>/widget.js). This file is a thin bootstrap: importing
 * unidtk-core.js registers every global listener, wires the widget registry,
 * and starts widget loops.
 */
import "./unidtk-core.js";
