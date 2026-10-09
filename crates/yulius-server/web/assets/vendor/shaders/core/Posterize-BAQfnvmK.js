import { Ac as defineStd, Lt as posterize, Ml as p } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Posterize",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Reduce color depth to create a poster effect",
	props: { intensity: {
		default: 5,
		description: "The intensity of the posterization effect (lower is more posterized)",
		ui: {
			type: ["range", "map"],
			min: 2,
			max: 20,
			step: 1,
			label: "Intensity",
			group: "Effect"
		}
	} },
	effect: posterize(p("intensity")),
	missingChildMessage: "You must pass a child component into the Posterize shader."
});
var Posterize_default = componentDefinition;
export { componentDefinition as n, Posterize_default as t };
