import { Ac as defineStd, Mt as grayscale } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Grayscale",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Convert colors to black and white",
	props: {},
	effect: grayscale(),
	missingChildMessage: "You must pass a child component into the Grayscale shader."
});
var Grayscale_default = componentDefinition;
export { componentDefinition as n, Grayscale_default as t };
