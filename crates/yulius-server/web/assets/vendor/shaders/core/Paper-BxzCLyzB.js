import { Ac as defineStd, Bd as ZERO, Hd as call, Jd as vec4, Td as paperSurface, if as unpremultiplyAlpha } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Paper",
	role: "filter",
	species: "custom",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Applies realistic paper grain and surface roughness to child content",
	requiresRTT: true,
	requiresChild: true,
	props: {
		roughness: {
			default: .3,
			description: "Surface roughness — higher values create more pronounced brightness variation",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Roughness",
				group: "Texture"
			}
		},
		grainScale: {
			default: 1,
			description: "Scale of the paper grain — lower = coarser, higher = finer",
			ui: {
				type: "range",
				min: .1,
				max: 3,
				step: .01,
				label: "Grain Scale",
				group: "Texture"
			}
		},
		displacement: {
			default: .15,
			description: "Surface micro-roughness — shifts pixels at grain scale like real paper fiber bumps",
			ui: {
				type: "range",
				min: 0,
				max: 1,
				step: .01,
				label: "Displacement",
				group: "Texture"
			}
		},
		seed: {
			default: 0,
			description: "Random seed for pattern variation",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Texture"
			}
		}
	},
	gpu: { fragment: (params) => {
		const { childNode, ctx, uniforms, convertToTexture } = params;
		if (!childNode) {
			console.error("You must pass a child component into the Paper shader.");
			return ZERO;
		}
		const texture = convertToTexture(childNode);
		const surface = call(paperSurface, "paperSurface", [
			ctx.uv,
			ctx.aspect,
			uniforms.grainScale,
			uniforms.seed,
			uniforms.displacement,
			uniforms.roughness
		]);
		const straight = call(unpremultiplyAlpha, "unpremultiplyAlpha", [texture.sample(surface.member("xy"))]);
		return vec4(straight.member("rgb").mul(surface.member("z")), straight.member("a"));
	} }
});
var Paper_default = componentDefinition;
export { componentDefinition as n, Paper_default as t };
