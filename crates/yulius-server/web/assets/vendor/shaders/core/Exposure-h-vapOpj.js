import { Ac as defineStd, Ml as p, kt as exposure, xs as isValue, ys as crosses } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Exposure",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Multiplicative exposure (gain) on the child. Unlike additive brightness, it scales values around black — blacks stay black while highlights can be pushed past 1.0 to feed HDR bloom and tone-mapping roll-off. Pointwise, so no extra render pass is needed.",
	props: { exposure: {
		default: 1,
		description: "Brightness multiplier (gain). 1 = unchanged, >1 pushes highlights past white for HDR bloom, <1 darkens.",
		recompile: crosses(1),
		ui: {
			type: ["range", "map"],
			min: 0,
			max: 8,
			step: .1,
			label: "Exposure",
			group: "Adjustments"
		}
	} },
	effect: exposure(p("exposure")),
	identityWhen: isValue("exposure", 1),
	missingChildMessage: "You must pass a child component into the Exposure shader."
});
var Exposure_default = componentDefinition;
export { componentDefinition as n, Exposure_default as t };
