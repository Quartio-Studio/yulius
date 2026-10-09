import { Ac as defineStd, Ml as p, Rt as saturate, xs as isValue, ys as crosses } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Saturation",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Adjust color saturation intensity",
	props: { intensity: {
		default: 1,
		description: "The intensity of the saturation effect (1 being no change)",
		recompile: crosses(1),
		ui: {
			type: ["range", "map"],
			min: 0,
			max: 3,
			step: .1,
			label: "Intensity",
			group: "Adjustments"
		}
	} },
	effect: saturate(p("intensity")),
	identityWhen: isValue("intensity", 1),
	missingChildMessage: "You must pass a child component into the Saturation shader."
});
var Saturation_default = componentDefinition;
export { componentDefinition as n, Saturation_default as t };
