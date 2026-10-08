import { Ac as defineStd, Ml as p, Nt as hueRotate, Ss as isZero, ys as crosses } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "HueShift",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Rotate hue around the color wheel",
	props: { shift: {
		default: 0,
		description: "The amount to shift the hue by",
		recompile: crosses(0),
		ui: {
			type: ["range", "map"],
			min: -180,
			max: 180,
			step: 1,
			label: "Shift",
			group: "Adjustments"
		}
	} },
	effect: hueRotate(p("shift")),
	identityWhen: isZero("shift"),
	missingChildMessage: "You must pass a child component into the Hue Shift shader."
});
var HueShift_default = componentDefinition;
export { componentDefinition as n, HueShift_default as t };
