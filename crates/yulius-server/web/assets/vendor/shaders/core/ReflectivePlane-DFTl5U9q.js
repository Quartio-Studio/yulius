import { Ac as defineStd, Bd as ZERO, Hd as call, Ml as p, dn as mirrorAcrossRow, if as unpremultiplyAlpha, nn as depthRampBlur, yn as planarReflection } from "./std-ClftfWpF.js";
import { s as transformEdges } from "./transformations-P50Uo_Ju.js";
const componentDefinition = defineStd({
	name: "ReflectivePlane",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Reflective floor that mirrors the content above it",
	requiresRTT: true,
	requiresChild: true,
	props: {
		height: {
			default: .7,
			description: "Vertical position of the reflective surface",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Floor Height",
				group: "Plane"
			}
		},
		distance: {
			default: .5,
			description: "How far below the floor the reflection remains visible before fully fading to transparent.",
			ui: {
				type: ["range", "map"],
				min: .01,
				max: 1,
				step: .01,
				label: "Reflection Distance",
				group: "Plane"
			}
		},
		falloff: {
			default: .5,
			description: "Width of the fade zone, as a fraction of reflection distance.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 3,
				step: .01,
				label: "Reflection Falloff",
				group: "Plane"
			}
		},
		blur: {
			default: 3,
			description: "Maximum blur applied to the reflection far from the surface.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 5,
				step: .01,
				label: "Blur",
				group: "Surface"
			}
		},
		blurDistance: {
			default: .3,
			description: "How far below the surface the blur takes to ramp from sharp to maximum.",
			ui: {
				type: "range",
				min: .01,
				max: 1,
				step: .01,
				label: "Blur Distance",
				group: "Surface"
			}
		},
		edges: {
			default: "stretch",
			description: "How to handle reflected samples that fall outside the source content.",
			transform: transformEdges,
			compileTime: true,
			ui: {
				type: "select",
				options: [
					{
						label: "Stretch",
						value: "stretch"
					},
					{
						label: "Transparent",
						value: "transparent"
					},
					{
						label: "Mirror",
						value: "mirror"
					},
					{
						label: "Wrap",
						value: "wrap"
					}
				],
				label: "Edges",
				group: "Surface"
			}
		}
	},
	...depthRampBlur({
		line: p("height"),
		blur: p("blur"),
		blurDistance: p("blurDistance"),
		halfKernel: 30
	}),
	gpu: { fragment: (params) => {
		const { childNode, ctx, computeOutputs, convertToTexture } = params;
		if (!childNode) return ZERO;
		const blurred = computeOutputs?.blurredTexture;
		const source = computeOutputs?.childTexture ?? convertToTexture(childNode);
		const reflectionTex = blurred ?? source;
		return call(unpremultiplyAlpha, "unpremultiplyAlpha", [planarReflection({
			line: p("height"),
			distance: p("distance"),
			falloff: p("falloff"),
			edges: p("edges")
		}, source.sample(ctx.uv), (uv) => reflectionTex.sample(uv), mirrorAcrossRow(p("height")))(params)]);
	} }
});
var ReflectivePlane_default = componentDefinition;
export { componentDefinition as n, ReflectivePlane_default as t };
