import { Ac as defineStd, Do as solidColor, Ml as p } from "./std-ClftfWpF.js";
import { a as transformColor } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "SolidColor",
	role: "generator",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Textures",
	description: "Fill the canvas with a single solid color",
	acceptsUVContext: true,
	props: { color: {
		default: "#5b18ca",
		transform: transformColor,
		description: "The solid color to display",
		ui: {
			type: "color",
			label: "Color",
			group: "Colors"
		}
	} },
	paint: solidColor(p("color"))
});
var SolidColor_default = componentDefinition;
export { componentDefinition as n, SolidColor_default as t };
