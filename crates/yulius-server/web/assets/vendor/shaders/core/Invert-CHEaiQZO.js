import { Ac as defineStd, Pt as invert } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Invert",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Invert RGB colors while preserving alpha",
	props: {},
	effect: invert(),
	missingChildMessage: "You must pass a child component into the Invert shader."
});
var Invert_default = componentDefinition;
export { componentDefinition as n, Invert_default as t };
