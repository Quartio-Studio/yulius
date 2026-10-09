import { Ac as defineStd, An as sharpen, Ml as p, Ss as isZero, ys as crosses } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Sharpness",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Adjust image sharpness using a convolution kernel",
	props: { sharpness: {
		default: 0,
		description: "How sharp to make the underlying image",
		recompile: crosses(0),
		ui: {
			type: ["range", "map"],
			min: 0,
			max: 5,
			step: .1,
			label: "Sharpness",
			group: "Adjustments"
		}
	} },
	identityWhen: isZero("sharpness"),
	effect: sharpen(p("sharpness"))
});
var Sharpness_default = componentDefinition;
export { componentDefinition as n, Sharpness_default as t };
