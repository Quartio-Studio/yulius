import { Ac as defineStd, Ht as vibrance, Ml as p, Ss as isZero, ys as crosses } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Vibrance",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Selective saturation adjustment protecting skin tones",
	props: { intensity: {
		default: 0,
		description: "The intensity of the vibrance effect",
		recompile: crosses(0),
		ui: {
			type: ["range", "map"],
			min: -2,
			max: 2,
			step: .1,
			label: "Intensity",
			group: "Adjustments"
		}
	} },
	effect: vibrance(p("intensity")),
	identityWhen: isZero("intensity"),
	missingChildMessage: "You must pass a child component into the Vibrance shader."
});
var Vibrance_default = componentDefinition;
export { componentDefinition as n, Vibrance_default as t };
